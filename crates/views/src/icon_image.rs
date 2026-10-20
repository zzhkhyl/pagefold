//! 把系统图标位图交给 GPUI 渲染。
//!
//! ## 为什么要自己写 PNG
//!
//! GPUI 的 `img()` 只接受三种东西：资源路径、已经解码好的 `RenderImage`、
//! 或者「编码后的字节 + 格式」（`Image::from_bytes`）。最后一种是唯一不需要
//! 依赖 gpui 内部类型构造方式的路径——`RenderImage` 需要 `image::Frame`，
//! 而 `image` 不是本项目（也不该成为本项目）的直接依赖。
//!
//! 因此这里把 RGBA 编成 PNG：**deflate 只使用「存储块」**（不压缩），
//! 编码器因此不需要任何压缩算法依赖，几十行就能写完且完全可测。
//! 32×32 的图标编码后约 4KB，一次编码、长期复用（见 [`image_for`] 的缓存）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use fm_core::IconId;
use fm_core::icon::RasterIcon;
use gpui::{Image, ImageFormat, ImageSource};

/// PNG 文件签名。
const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// 取某个图标句柄对应的可渲染图像。
///
/// 首次调用会编码 PNG 并缓存 `Arc<Image>`；之后每次命中都是哈希查找 + 一次
/// `Arc` 克隆。**缓存键是 `IconId`**：图标本身按扩展名共享（见
/// `fm_services::icon::IconCache`），所以这里的条目数与扩展名种类同阶，不会随目录大小增长。
///
/// 返回 `None` 表示该条目没有系统图标（`IconId(0)` 或尚未登记），
/// 调用方应退回内置 SVG 图标。
pub fn image_for(id: IconId) -> Option<ImageSource> {
    if id == IconId(0) {
        return None;
    }

    let mut cache = lock();
    if let Some(cached) = cache.get(&id.0) {
        return Some(ImageSource::Image(Arc::clone(cached)));
    }

    // 句柄可能还没登记（例如扫描时提取失败、渲染时才知道）。
    // 这种情况不写缓存：下次登记成功后仍有机会取到图。
    let raster = fm_core::icon::resolve(id)?;
    let png = encode_png(&raster);
    let image = Arc::new(Image::from_bytes(ImageFormat::Png, png));
    cache.insert(id.0, Arc::clone(&image));
    Some(ImageSource::Image(image))
}

type Cache = Mutex<HashMap<u32, Arc<Image>>>;

fn lock() -> std::sync::MutexGuard<'static, HashMap<u32, Arc<Image>>> {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    // 纯缓存：中毒后继续用，不让一次渲染路径上的 panic 连累后续所有图标。
    CACHE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 已缓存的图像数量。供测试与诊断使用。
pub fn cached_images() -> usize {
    lock().len()
}

/// 把 RGBA 位图编码成 PNG（无压缩 deflate 存储块）。
///
/// 产物是合法的 PNG（8 位 RGBA、无交错、filter 全 0），
/// 任何标准解码器都能读——包括 GPUI 内部用的 `image` crate。
pub fn encode_png(icon: &RasterIcon) -> Vec<u8> {
    let width = icon.width as usize;
    let row_bytes = width * 4;

    // 每条扫描线前面要加一个 filter 字节（0 = None）。
    let mut raw = Vec::with_capacity((row_bytes + 1) * icon.height as usize);
    for row in icon.rgba.chunks_exact(row_bytes.max(1)) {
        if row.len() != row_bytes {
            // `RasterIcon` 已保证长度整除，这里的防御只为不在渲染路径上 panic。
            break;
        }
        raw.push(0);
        raw.extend_from_slice(row);
    }

    let mut png = Vec::with_capacity(raw.len() + 64);
    png.extend_from_slice(&PNG_SIGNATURE);
    write_chunk(&mut png, b"IHDR", &{
        let mut data = Vec::with_capacity(13);
        data.extend_from_slice(&icon.width.to_be_bytes());
        data.extend_from_slice(&icon.height.to_be_bytes());
        // 位深 8、颜色类型 6（RGBA）、压缩/滤波/交错均为 0。
        data.extend_from_slice(&[8, 6, 0, 0, 0]);
        data
    });
    write_chunk(&mut png, b"IDAT", &zlib_stored(&raw));
    write_chunk(&mut png, b"IEND", &[]);
    png
}

/// 写一个 PNG 块：长度 + 类型 + 数据 + CRC（CRC 覆盖类型与数据）。
fn write_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc_input = Vec::with_capacity(4 + data.len());
    crc_input.extend_from_slice(kind);
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

/// zlib 流（RFC 1950）包一层「存储块」deflate 数据。
///
/// 存储块最大 65535 字节；`BFINAL` 只在最后一块置 1。
fn zlib_stored(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len() + 16);
    // CMF=0x78（deflate, 32K 窗口）、FLG=0x01（校验位正确、无字典）。
    out.push(0x78);
    out.push(0x01);

    let mut remaining = raw;
    loop {
        let take = remaining.len().min(u16::MAX as usize);
        let last = take == remaining.len();
        out.push(u8::from(last));
        out.extend_from_slice(&(take as u16).to_le_bytes());
        out.extend_from_slice(&(!(take as u16)).to_le_bytes());
        out.extend_from_slice(&remaining[..take]);
        remaining = &remaining[take..];
        if last {
            break;
        }
    }

    out.extend_from_slice(&adler32(raw).to_be_bytes());
    out
}

/// CRC-32（PNG 用的 IEEE 802.3 多项式，反射实现）。
///
/// 按位计算而不是查表：只在编码图标时跑一次，几 KB 数据下差异不可见，
/// 省掉一张 1KB 的静态表。
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// Adler-32（zlib 的校验和）。
pub fn adler32(bytes: &[u8]) -> u32 {
    const MOD: u32 = 65_521;
    let mut a = 1u32;
    let mut b = 0u32;
    for &byte in bytes {
        a = (a + u32::from(byte)) % MOD;
        b = (b + a) % MOD;
    }
    (b << 16) | a
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: u32, height: u32, rgba: [u8; 4]) -> RasterIcon {
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..width * height {
            pixels.extend_from_slice(&rgba);
        }
        RasterIcon::new(width, height, pixels).unwrap()
    }

    #[test]
    fn crc32_matches_the_standard_check_value() {
        // IEEE 802.3 的公开校验值：任何实现都必须给出这个数。
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn adler32_matches_the_published_example() {
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }

    #[test]
    fn encoded_png_has_signature_and_required_chunks() {
        let png = encode_png(&solid(2, 2, [10, 20, 30, 255]));
        assert_eq!(&png[..8], &PNG_SIGNATURE, "PNG 签名必须完全一致");

        // 块结构：长度(4) + 类型(4) + 数据 + CRC(4)。
        let mut offset = 8;
        let mut kinds = Vec::new();
        while offset + 8 <= png.len() {
            let length = u32::from_be_bytes(png[offset..offset + 4].try_into().unwrap()) as usize;
            let kind = &png[offset + 4..offset + 8];
            kinds.push(String::from_utf8_lossy(kind).into_owned());

            // 顺带校验每个块的 CRC：手写编码器最容易错的就是这里。
            let data_start = offset + 8;
            let crc_at = data_start + length;
            let expected = crc32(&png[offset + 4..crc_at]);
            let actual = u32::from_be_bytes(png[crc_at..crc_at + 4].try_into().unwrap());
            assert_eq!(actual, expected, "{} 块的 CRC 不匹配", kinds.last().unwrap());
            offset = crc_at + 4;
        }
        assert_eq!(kinds, vec!["IHDR", "IDAT", "IEND"]);
    }

    #[test]
    fn idat_starts_with_a_valid_zlib_header() {
        let png = encode_png(&solid(1, 1, [0, 0, 0, 0]));
        // 定位 IDAT 的数据区：签名 8 + IHDR(4+4+13+4) + IDAT 长度/类型 8。
        let idat_data = 8 + 25 + 8;
        assert_eq!(png[idat_data], 0x78, "CMF 必须是 0x78");
        assert_eq!(png[idat_data + 1], 0x01, "FLG 必须让 (CMF*256+FLG) % 31 == 0");
        assert_eq!(
            (u16::from(png[idat_data]) * 256 + u16::from(png[idat_data + 1])) % 31,
            0
        );
    }

    #[test]
    fn large_images_are_split_into_multiple_stored_blocks() {
        // 200×200 的原始数据约 160KB，必须被切成多个 <=65535 的存储块，
        // 否则 LEN/NLEN 会溢出成错误的长度。
        let icon = solid(200, 200, [1, 2, 3, 4]);
        let png = encode_png(&icon);
        assert!(png.len() > 160_000, "未压缩编码的数据量应当接近原始大小");
        // 能被解码器接受的前提是块结构自洽，这里用 chunk 遍历做一次结构校验。
        let mut offset = 8;
        let mut seen_idat = false;
        while offset + 8 <= png.len() {
            let length = u32::from_be_bytes(png[offset..offset + 4].try_into().unwrap()) as usize;
            let kind = &png[offset + 4..offset + 8];
            if kind == b"IDAT" {
                seen_idat = true;
            }
            offset += 12 + length;
        }
        assert!(seen_idat);
        assert_eq!(offset, png.len(), "块长度必须恰好走到文件末尾");
    }

    #[test]
    fn zero_id_never_produces_an_image() {
        // `IconId(0)` 是「没有系统图标」的哨兵，必须走 SVG 兜底。
        assert!(image_for(IconId(0)).is_none());
    }
}
