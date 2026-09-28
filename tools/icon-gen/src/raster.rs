//! SVG → 位图。
//!
//! 用 `resvg` / `usvg` 栅格化：这是 GPUI 渲染 SVG 时用的同一套引擎，
//! 因此「矢量源长什么样」与「各尺寸 PNG 长什么样」不会出现两套解释。
//!
//! 每个尺寸都是**独立栅格化**（在目标分辨率下重新求一遍几何），
//! 而不是把 1024 的位图缩小——后者在 16px 会先糊掉再缩，边缘发灰。

use anyhow::{Context, Result};
use resvg::tiny_skia::{Pixmap, Transform};
use resvg::usvg;

use crate::design;

/// 已解析的矢量图。
pub struct Vector {
    tree: usvg::Tree,
}

impl Vector {
    /// 解析 SVG 文本。
    pub fn parse(svg: &str) -> Result<Self> {
        let options = usvg::Options {
            // 交付的 SVG 一定带 `width/height="1024"`；这里再兜一层默认尺寸，
            // 保证即使属性缺失，缩放推算仍按 1024 画布进行。
            default_size: usvg::Size::from_wh(design::CANVAS, design::CANVAS)
                .expect("1024×1024 是合法尺寸"),
            ..usvg::Options::default()
        };
        let tree = usvg::Tree::from_str(svg, &options)
            .with_context(|| format!("SVG 解析失败（前 80 字符：{}）", head(svg)))?;
        Ok(Self { tree })
    }

    /// 按目标边长渲染成 RGBA 位图（预乘 alpha）。
    pub fn render(&self, size: u32) -> Result<Pixmap> {
        let mut pixmap = Pixmap::new(size, size)
            .with_context(|| format!("{size}×{size} 位图分配失败（尺寸为 0？）"))?;
        let source = self.tree.size().width();
        let scale = size as f32 / source;
        resvg::render(
            &self.tree,
            Transform::from_scale(scale, scale),
            &mut pixmap.as_mut(),
        );
        Ok(pixmap)
    }
}

/// 取某个像素的**直通 alpha** RGBA（0..=255）。
pub fn pixel(pixmap: &Pixmap, x: u32, y: u32) -> [u8; 4] {
    let color = pixmap.pixels()[(y * pixmap.width() + x) as usize].demultiply();
    [
        color.red(),
        color.green(),
        color.blue(),
        color.alpha(),
    ]
}

/// 整幅是否完全不透明。
pub fn is_fully_opaque(pixmap: &Pixmap) -> bool {
    pixmap.pixels().iter().all(|pixel| pixel.alpha() == 255)
}

/// 某个像素的 alpha。
pub fn alpha(pixmap: &Pixmap, x: u32, y: u32) -> u8 {
    pixmap.pixels()[(y * pixmap.width() + x) as usize].alpha()
}

/// 直通 alpha 的 RGB 缓冲（丢掉 alpha 通道）。
pub fn rgb_buffer(pixmap: &Pixmap) -> Vec<u8> {
    let mut buffer = Vec::with_capacity((pixmap.width() * pixmap.height() * 3) as usize);
    for pixel in pixmap.pixels() {
        let color = pixel.demultiply();
        buffer.push(color.red());
        buffer.push(color.green());
        buffer.push(color.blue());
    }
    buffer
}

/// 编码为 RGBA PNG（Web / Android / 通用场景）。
pub fn png_rgba(pixmap: &Pixmap) -> Result<Vec<u8>> {
    pixmap.encode_png().context("PNG 编码失败")
}

/// 编码为 **RGB** PNG（无 alpha 通道）。
///
/// iOS 的 AppIcon 不允许带 alpha 通道——只把 alpha 全部写成 255 还不够，
/// 部分校验会直接检查颜色类型，因此这里输出 color type 2（RGB）的 PNG。
pub fn png_rgb(pixmap: &Pixmap) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, pixmap.width(), pixmap.height());
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().context("PNG 头写入失败")?;
        writer
            .write_image_data(&rgb_buffer(pixmap))
            .context("PNG 数据写入失败")?;
        writer.finish().context("PNG 收尾失败")?;
    }
    Ok(out)
}

/// ICO 内嵌的 BMP 载荷（32 位 BGRA + AND 掩码）。
///
/// ICO 里的位图与 DIB 不同：像素是**直通 alpha**（非预乘），且必须附一段 1bpp
/// 的 AND 掩码。掩码在位图带 alpha 时被忽略，但老程序会读它，因此按惯例写全 0。
pub fn bmp_payload(pixmap: &Pixmap) -> Vec<u8> {
    let width = pixmap.width();
    let height = pixmap.height();
    let mut out = Vec::with_capacity((width * height * 4 + height * 8 + 40) as usize);

    // BITMAPINFOHEADER
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(width as i32).to_le_bytes());
    // ICO 的高度是实际高度的两倍：BMP 部分 + AND 掩码部分。
    out.extend_from_slice(&((height * 2) as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // planes
    out.extend_from_slice(&32u16.to_le_bytes()); // bpp
    out.extend_from_slice(&0u32.to_le_bytes()); // 不压缩
    out.extend_from_slice(&(width * height * 4).to_le_bytes()); // 图像数据大小
    out.extend_from_slice(&[0u8; 16]); // 分辨率与调色板信息，图标里不使用

    // 像素：自下而上、BGRA。
    for y in (0..height).rev() {
        for x in 0..width {
            let [red, green, blue, alpha] = pixel(pixmap, x, y);
            out.extend_from_slice(&[blue, green, red, alpha]);
        }
    }

    // AND 掩码：1bpp、自下而上、每行按 4 字节对齐，全 0（表示「不透明由 alpha 决定」）。
    let mask_stride = width.div_ceil(32) * 4;
    out.resize(out.len() + (mask_stride * height) as usize, 0);
    out
}

/// 取开头若干字符，便于在报错里指出是哪个 SVG 解析失败。
fn head(text: &str) -> String {
    text.chars().take(80).collect::<String>().replace('\n', " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solid_vector_renders_a_transparent_corner_and_an_opaque_plate() {
        // 超级椭圆不覆盖画布四角，因此角上必须是透明的。
        let vector = Vector::parse(&design::Spec::solid().svg("t")).unwrap();
        let pixmap = vector.render(64).unwrap();
        assert_eq!(alpha(&pixmap, 1, 1), 0, "左上角应透明");
        assert_eq!(alpha(&pixmap, 32, 32), 255, "画布中心应不透明");
    }

    #[test]
    fn square_vector_covers_the_whole_canvas() {
        let vector = Vector::parse(&design::Spec::square(design::ACCENT, design::WHITE, 0.52).svg("t"))
            .unwrap();
        let pixmap = vector.render(32).unwrap();
        assert!(is_fully_opaque(&pixmap), "满幅方形必须整幅不透明");
    }

    #[test]
    fn png_rgb_has_no_alpha_channel() {
        let vector = Vector::parse(&design::Spec::square(design::ACCENT, design::WHITE, 0.52).svg("t"))
            .unwrap();
        let pixmap = vector.render(16).unwrap();
        let bytes = png_rgb(&pixmap).unwrap();
        // PNG 的 IHDR 第 9 字节（偏移 25）是颜色类型：2 = RGB，6 = RGBA。
        assert_eq!(bytes[25], 2, "iOS 产物必须是 RGB（无 alpha 通道）");
        let rgba = png_rgba(&pixmap).unwrap();
        assert_eq!(rgba[25], 6, "iOS 之外的产物保持 RGBA 以便叠在任何底色上");
    }

    #[test]
    fn bmp_payload_matches_the_icon_bitmap_layout() {
        let vector = Vector::parse(&design::Spec::solid().svg("t")).unwrap();
        let pixmap = vector.render(16).unwrap();
        let payload = bmp_payload(&pixmap);
        // 头 40 字节 + 像素 16×16×4 + 掩码 4×16（每行 4 字节对齐）。
        assert_eq!(payload.len(), 40 + 16 * 16 * 4 + 4 * 16);
        // 高度字段是实际高度的两倍。
        let height = i32::from_le_bytes([payload[8], payload[9], payload[10], payload[11]]);
        assert_eq!(height, 32);
    }
}
