//! 文件缩略图：`IShellItemImageFactory`。
//!
//! 与 [`super::icon`] 的分工：
//!
//! | | 输入 | 尺寸 | 成本 |
//! | --- | --- | --- | --- |
//! | `icon::system_icon` | **扩展名**（一种类型一张） | 32×32 | 低，且天然去重 |
//! | 本模块 | **具体文件路径**（一文件一张） | 最长边 ≤ 请求值 | 高，需要真去解码 |
//!
//! 因此缩略图必须异步取（见 `fm_services::thumbnail`），不能像图标那样在渲染路径上现要。
//!
//! 走 shell 而不是自己解码图片，理由与回收站那节相同：图片只是最省事的一类，
//! 视频封面、Office 文档首页、PDF 首页、甚至文件夹预览都由同一套处理器提供；
//! 自己接一套解码器只能覆盖其中一小块，还要额外引入一堆依赖。

use std::os::windows::ffi::OsStrExt as _;
use std::path::Path;

use fm_core::RasterIcon;
use windows::Win32::Foundation::SIZE;
use windows::Win32::Graphics::Gdi::{
    BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits,
    GetObjectW, HGDIOBJ, ReleaseDC,
};
use windows::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
};
use windows::Win32::UI::Shell::{
    IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_RESIZETOFIT,
};
use windows::core::PCWSTR;

/// 取一个文件的缩略图，最长边不超过 `size`。
///
/// 失败返回 `None`：没有预览（文本、未知类型）、文件被占用、内容加密……
/// 都会失败，调用方一律退回扩展名图标，而不是显示一个破图。
///
/// **必须在线程池/工作线程上调用**：解码一张大图可能几十毫秒，
/// 放在渲染路径上会让滚动掉帧。
pub fn thumbnail(path: &Path, size: u32) -> Option<RasterIcon> {
    if size == 0 {
        return None;
    }

    unsafe {
        // 每个调用线程各自保证 COM 可用。`S_FALSE`（本线程已初始化）也是成功；
        // 失败（例如线程已被初始化成 MTA）时仍然继续尝试——这些 shell API
        // 在两种套间里都能用，失败只意味着需要配对调用 `CoUninitialize`。
        let initialized = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
        let result = thumbnail_inner(path, size);
        if initialized.is_ok() {
            CoUninitialize();
        }
        result
    }
}

unsafe fn thumbnail_inner(path: &Path, size: u32) -> Option<RasterIcon> {
    // shell 的解析函数要求**绝对路径**：传相对路径时 `SHCreateItemFromParsingName`
    // 直接失败（返回 None），表现为「缩略图一个都不出现」。
    // 从命令行用相对目录启动时，扫描出来的条目路径就是相对的，因此这里统一补全。
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());

    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let factory: IShellItemImageFactory =
        unsafe { SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None) }.ok()?;

    // `RESIZETOFIT` 保持长宽比（等比缩到框内）。
    //
    // **不加 `BIGGERSIZEOK`**：那个标志允许 shell 返回比请求更大的图（命中缓存时
    // 常常是 256px，某些处理器甚至直接给原图）。一张 4000×3000 的照片按 RGBA
    // 展开就是 48MB，几百张目录足以把内存吃光，而我们最多只画到 192px。
    let bitmap = unsafe {
        factory.GetImage(
            SIZE {
                cx: size as i32,
                cy: size as i32,
            },
            SIIGBF_RESIZETOFIT,
        )
    }
    .ok()?;

    let raster = unsafe { bitmap_to_raster(bitmap) };
    let _ = unsafe { DeleteObject(HGDIOBJ(bitmap.0)) };
    raster
}

/// 把 32bpp 位图读成 RGBA。
unsafe fn bitmap_to_raster(bitmap: windows::Win32::Graphics::Gdi::HBITMAP) -> Option<RasterIcon> {
    let mut info = BITMAP::default();
    let copied = unsafe {
        GetObjectW(
            HGDIOBJ(bitmap.0),
            std::mem::size_of::<BITMAP>() as i32,
            Some(&mut info as *mut BITMAP as *mut std::ffi::c_void),
        )
    };
    if copied == 0 || info.bmWidth <= 0 || info.bmHeight <= 0 {
        return None;
    }
    let (width, height) = (info.bmWidth, info.bmHeight);

    // 负高度 = 自上而下存储，省掉一次垂直翻转。
    let mut header = BITMAPINFO {
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

    let count = (width as usize).checked_mul(height as usize)?;
    let mut buffer = vec![0u8; count.checked_mul(4)?];

    let screen = unsafe { GetDC(None) };
    let lines = unsafe {
        GetDIBits(
            screen,
            bitmap,
            0,
            height as u32,
            Some(buffer.as_mut_ptr() as *mut std::ffi::c_void),
            &mut header,
            DIB_RGB_COLORS,
        )
    };
    unsafe { ReleaseDC(None, screen) };
    if lines == 0 {
        return None;
    }

    // DIB 里是 BGRA；RGBA 需要交换 0/2 两个通道。
    let mut rgba = Vec::with_capacity(count * 4);
    let mut all_transparent = true;
    for pixel in buffer.as_chunks::<4>().0 {
        if pixel[3] != 0 {
            all_transparent = false;
        }
        rgba.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
    }

    // 有些来源（非 DIB 的设备相关位图）不带 alpha 通道，读出来整片是 0。
    // 直接交给渲染层会得到一张完全透明的图，看起来就是「缩略图没加载」。
    if all_transparent {
        for pixel in rgba.as_chunks_mut::<4>().0 {
            pixel[3] = 255;
        }
    }

    RasterIcon::new(width as u32, height as u32, rgba)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 手写一张 4×4 的 24 位 BMP：不引入任何图片依赖，但足以让 shell 认出
    /// 「这是张图片」并返回缩略图。
    fn write_bmp(path: &Path, width: u32, height: u32) {
        let row_bytes = width * 3;
        // 每行按 4 字节对齐。
        let padding = (4 - row_bytes % 4) % 4;
        let image_size = (row_bytes + padding) * height;
        let file_size = 54 + image_size;

        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"BM");
        bytes.extend_from_slice(&file_size.to_le_bytes());
        bytes.extend_from_slice(&[0, 0, 0, 0]);
        bytes.extend_from_slice(&54u32.to_le_bytes());

        bytes.extend_from_slice(&40u32.to_le_bytes());
        bytes.extend_from_slice(&(width as i32).to_le_bytes());
        bytes.extend_from_slice(&(height as i32).to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&24u16.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&image_size.to_le_bytes());
        bytes.extend_from_slice(&[0u8; 16]);

        for y in 0..height {
            for x in 0..width {
                // 对角线上一块红色，其余蓝色：任何通道顺序错乱都会改变「红多还是蓝多」。
                if x == y {
                    bytes.extend_from_slice(&[0, 0, 255]);
                } else {
                    bytes.extend_from_slice(&[255, 0, 0]);
                }
            }
            bytes.extend(std::iter::repeat_n(0u8, padding as usize));
        }

        std::fs::write(path, bytes).expect("写入测试用 BMP");
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("pagefold-thumb-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("创建临时目录");
        dir
    }

    #[test]
    fn thumbnail_of_a_real_image_has_pixels_and_fits_the_requested_box() {
        let dir = scratch("basic");
        let file = dir.join("block.bmp");
        write_bmp(&file, 64, 48);

        let raster = thumbnail(&file, 64).expect("BMP 应当能取到缩略图");

        assert!(raster.width <= 64 && raster.height <= 64, "不能超过请求的框");
        assert_eq!(raster.rgba.len(), (raster.width * raster.height * 4) as usize);
        // 不能是全透明（那等于没取到）也不能是纯色（那说明只拿到一张底板）。
        let colors: std::collections::HashSet<[u8; 3]> = raster
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .map(|pixel| [pixel[0], pixel[1], pixel[2]])
            .collect();
        assert!(colors.len() >= 2, "缩略图应当包含不止一种颜色");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_without_a_preview_handler_falls_back_to_its_icon() {
        let dir = scratch("text");
        let file = dir.join("note.txt");
        std::fs::write(&file, b"hello").expect("写入文本");

        // **重要行为**：没有预览处理器的文件，shell 会退回它的图标而不是失败。
        // 因此调用方不能「对所有文件都请求缩略图」——那等于用一张高分辨率图标
        // 替换我们已有的、按扩展名去重过的图标，既慢又更模糊。
        // 挑选白名单是 `fm_services::thumbnail::is_previewable` 的职责。
        let raster = thumbnail(&file, 64).expect("shell 会退回图标，而不是返回 None");
        assert!(raster.width <= 64 && raster.height <= 64);
        assert_eq!(raster.rgba.len(), (raster.width * raster.height * 4) as usize);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
