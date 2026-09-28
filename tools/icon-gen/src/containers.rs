//! 容器格式：Windows `.ico` 与 macOS `.icns`。
//!
//! 两种容器都只是「若干张位图 + 一段索引」，没有压缩算法要写，
//! 因此直接手写而不是引入 `ico` / `icns` 这类只为一个用途存在的依赖。
//!
//! - `.ico`：≤64px 用 BMP（32 位 BGRA + AND 掩码），≥128px 用 PNG。
//!   这是 Windows 上兼容性最好的组合：老程序读 BMP，新外壳走 PNG。
//! - `.icns`：全部用 PNG 载荷（macOS 10.7+ 支持），
//!   覆盖 16/32/64/128/256/512/1024 及其 @2x 槽位。

use anyhow::{Result, bail};

/// ICO 中的一个图像条目。
pub struct IcoImage {
    /// 边长（像素），`256` 在目录里编码为 `0`。
    pub size: u32,
    /// 载荷：BMP 或 PNG。
    pub payload: Vec<u8>,
}

/// 组装 ICO 文件。
pub fn write_ico(images: &[IcoImage]) -> Result<Vec<u8>> {
    if images.is_empty() {
        bail!("ICO 至少要有一张图像");
    }
    let mut out = Vec::new();

    // ICONDIR
    out.extend_from_slice(&0u16.to_le_bytes()); // reserved
    out.extend_from_slice(&1u16.to_le_bytes()); // type = icon
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());

    // 目录项之后紧跟全部载荷，因此偏移要先算出来。
    let mut offset = 6 + images.len() as u32 * 16;
    for image in images {
        let dimension = if image.size >= 256 {
            0
        } else {
            image.size as u8
        };
        out.push(dimension); // 宽
        out.push(dimension); // 高
        out.push(0); // 调色板颜色数（32 位图不用）
        out.push(0); // reserved
        out.extend_from_slice(&1u16.to_le_bytes()); // planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bpp
        out.extend_from_slice(&(image.payload.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += image.payload.len() as u32;
    }

    for image in images {
        out.extend_from_slice(&image.payload);
    }
    Ok(out)
}

/// ICNS 的一个数据块。
pub struct IcnsChunk {
    /// 四字符类型码，例如 `b"ic09"`。
    pub kind: [u8; 4],
    /// PNG 载荷。
    pub payload: Vec<u8>,
}

/// 组装 ICNS 文件。
pub fn write_icns(chunks: &[IcnsChunk]) -> Result<Vec<u8>> {
    if chunks.is_empty() {
        bail!("ICNS 至少要有一个数据块");
    }

    let mut out = Vec::new();
    out.extend_from_slice(b"icns");
    // 总长度稍后回填：先占位。
    out.extend_from_slice(&0u32.to_be_bytes());

    for chunk in chunks {
        // 数据块长度包含 8 字节头自身。
        let length = 8 + chunk.payload.len() as u32;
        out.extend_from_slice(&chunk.kind);
        out.extend_from_slice(&length.to_be_bytes());
        out.extend_from_slice(&chunk.payload);
    }

    let total = out.len() as u32;
    out[4..8].copy_from_slice(&total.to_be_bytes());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ico_directory_points_at_the_payloads() {
        let images = vec![
            IcoImage {
                size: 16,
                payload: vec![0xaa; 40],
            },
            IcoImage {
                size: 256,
                payload: vec![0xbb; 32],
            },
        ];
        let bytes = write_ico(&images).unwrap();

        // ICONDIR：3 个字段 + 2 个目录项。
        assert_eq!(u16::from_le_bytes([bytes[2], bytes[3]]), 1, "type 必须是 1");
        assert_eq!(u16::from_le_bytes([bytes[4], bytes[5]]), 2, "应有 2 张图");

        // 第一项：宽高 16，偏移指向第 6+32 字节。
        assert_eq!(bytes[6], 16);
        let first_offset = u32::from_le_bytes([bytes[18], bytes[19], bytes[20], bytes[21]]);
        assert_eq!(first_offset, 6 + 32);

        // 第二项：256 在目录里写 0。
        assert_eq!(bytes[22], 0);
        let second_offset = u32::from_le_bytes([bytes[34], bytes[35], bytes[36], bytes[37]]);
        assert_eq!(second_offset, 6 + 32 + 40);
        assert_eq!(bytes.len(), 6 + 32 + 40 + 32);
    }

    #[test]
    fn icns_lengths_cover_header_and_payload() {
        let chunks = vec![
            IcnsChunk {
                kind: *b"ic07",
                payload: vec![1; 8],
            },
            IcnsChunk {
                kind: *b"ic09",
                payload: vec![2; 4],
            },
        ];
        let bytes = write_icns(&chunks).unwrap();
        assert_eq!(&bytes[0..4], b"icns");
        assert_eq!(
            u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize,
            bytes.len()
        );
        // 第一个块：类型 + 长度 8+8。
        assert_eq!(&bytes[8..12], b"ic07");
        assert_eq!(
            u32::from_be_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]),
            16
        );
    }

    #[test]
    fn empty_input_is_rejected() {
        assert!(write_ico(&[]).is_err());
        assert!(write_icns(&[]).is_err());
    }
}
