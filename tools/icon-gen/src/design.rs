//! 设计稿 C 方案（层叠文件夹）的几何、配色与 SVG 生成。
//!
//! 所有几何与色值逐条来自 `design/app-icons.html`，没有一处是重新构造的：
//!
//! - **底板**：`n = 5`、半宽 412 的超级椭圆（画布 100..924），等效圆角 22.1%；
//! - **标记**：两条文件夹路径，组变换 `translate(512 512) scale(1.08) translate(-512 -512)`；
//! - **描边**：设计稿写 20（组内单位），等效画布单位 `20 × 1.08 = 21.6`；
//! - **强调色**：`oklch(55% 0.2 258)`，设计稿公布的 sRGB 换算值是 `#076be3`。
//!
//! 设计稿的另一条硬约束是**安全区**：标志宽度不超过画布的 55%。
//! 本模块把它写成了代码里的夹取（[`Spec::clamp_mark_scale`]），
//! 这样任何「小尺寸加固」都不可能越界——而不是靠肉眼看图确认。
//!
//! 一份几何 + 参数化变体，而不是维护多份 SVG：底板形态（超级椭圆 / 方形 / 圆形 / 无）、
//! 缝隙处理（底板色描边 / 镂空）、标记缩放与描边宽度，全部是 [`Spec`] 的字段。

/// 画布边长。设计稿的所有坐标都写在这块 1024 画布上。
pub const CANVAS: f32 = 1024.0;

/// 超级椭圆底板路径。逐字节抄自 `design/app-icons.html` 的 `#plate`。
pub const PLATE_PATH: &str = "M924 512C924 574.4 925 635.4 922.5 697.3C920 756.5 915.1 826.3 870.7 870.7C826.2 915.1 756.4 920 697.2 922.5C629.6 925.3 586.6 924 499.6 924C464.1 924 381.9 924.8 326.7 922.5C267.5 920 197.7 915.1 153.3 870.7C108.9 826.2 104 756.4 101.5 697.2C98.7 629.6 100 586.6 100 499.6C100 464.1 99.2 381.9 101.5 326.7C104 267.5 108.9 197.7 153.3 153.3C197.8 108.9 267.6 104 326.8 101.5C394.4 98.7 437.4 100 524.4 100C559.9 100 642.1 99.2 697.3 101.5C756.5 104 826.3 108.9 870.7 153.3C915.1 197.8 920 267.6 922.5 326.8C925 387.7 924 451.8 924 512Z";

/// 后层文件夹（左上方那张）。
pub const FOLDER_BACK: &str = "M346 302H454L510 362H614A44 44 0 0 1 658 406V614A44 44 0 0 1 614 658H346A44 44 0 0 1 302 614V346A44 44 0 0 1 346 302Z";

/// 前层文件夹（右下那张，压在后面一张之上）。
pub const FOLDER_FRONT: &str = "M410 366H518L574 426H678A44 44 0 0 1 722 470V678A44 44 0 0 1 678 722H410A44 44 0 0 1 366 678V410A44 44 0 0 1 410 366Z";

/// 设计稿的标记缩放。
pub const DESIGN_MARK_SCALE: f32 = 1.08;

/// 设计稿的描边宽度，换算到画布单位（`20 × 1.08`）。
pub const DESIGN_STROKE: f32 = 21.6;

/// 两条路径的并集包围盒边长（不含描边）：x 与 y 都是 302..722。
const MARK_BOX: f32 = 420.0;

/// 画布中心。
const CENTER: f32 = CANVAS / 2.0;

/// 设计稿规定的安全区：标志宽度不超过画布的 55%。
pub const SAFE_ZONE_RATIO: f32 = 0.55;

/// 触发「小尺寸加固」的最大像素尺寸。
pub const SMALL_SIZE_MAX: u32 = 32;

/// 小尺寸加固的目标描边宽度（物理像素）。
///
/// 取 0.9px 而不是 1.0px：整数像素的线在 16px 下会显得像「画歪了的一格」，
/// 略低于 1px 既保证可辨识，又能保留抗锯齿带来的柔和过渡。
pub const SMALL_STROKE_PX: f32 = 0.9;

/// 小尺寸加固时标记的放大倍数。
pub const SMALL_MARK_BOOST: f32 = 1.08;

/// sRGB 颜色。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    /// `#rrggbb` 形式，用于 SVG 属性。
    pub fn hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.0, self.1, self.2)
    }
}

/// 强调色 `oklch(55% 0.2 258)`。
pub const ACCENT: Rgb = Rgb(0x07, 0x6b, 0xe3);
/// 纯白：实底下的标记颜色。
pub const WHITE: Rgb = Rgb(0xff, 0xff, 0xff);
/// `--ic-tint-plate`：浅底底板。
pub const TINT_PLATE: Rgb = Rgb(0xe4, 0xef, 0xff);
/// `--ic-tint-edge`：浅底描边。
pub const TINT_EDGE: Rgb = Rgb(0xc4, 0xd9, 0xf9);

/// `--ic-deep-plate` = `oklch(24% 0.05 258)`。
///
/// 设计稿的软色令牌（`--ic-tint-soft` / `--ic-deep-soft`）没有出现在这里：
/// 它们专门服务 B 方案那两枚页签，而 C 方案里没有任何「有功能含义的软色元素」。
pub fn deep_plate() -> Rgb {
    oklch_to_rgb(0.24, 0.05, 258.0)
}

/// OKLCH → sRGB。系数取自 Björn Ottosson 的 OKLab 定义，与 CSS `oklch()` 一致。
///
/// 设计稿只给了 OKLCH 值（深色底板与软色），而 SVG 需要 sRGB，
/// 因此这里把同一条换算链实现了一遍——而不是手工查一个颜色再抄进来。
pub fn oklch_to_rgb(lightness: f32, chroma: f32, hue: f32) -> Rgb {
    let hue = hue.to_radians();
    let a = chroma * hue.cos();
    let b = chroma * hue.sin();

    let l = lightness + 0.396_337_78 * a + 0.215_803_76 * b;
    let m = lightness - 0.105_561_346 * a - 0.063_854_17 * b;
    let s = lightness - 0.089_484_18 * a - 1.291_485_5 * b;

    let (l, m, s) = (l * l * l, m * m * m, s * s * s);

    let red = 4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s;
    let green = -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s;
    let blue = -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s;

    Rgb(encode(red), encode(green), encode(blue))
}

/// 线性 → gamma 编码，并夹到 0..=255。
fn encode(value: f32) -> u8 {
    let value = value.clamp(0.0, 1.0);
    let srgb = if value <= 0.003_130_8 {
        12.92 * value
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    };
    (srgb.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// 底板形态。
#[derive(Clone, Copy, Debug)]
pub enum Plate {
    /// 设计稿的超级椭圆底板；`edge` 是浅底方案用的 1px 描边（画布单位 4）。
    Superellipse { fill: Rgb, edge: Option<(Rgb, f32)> },
    /// 满幅直角方形：iOS 由系统裁圆角，因此必须整幅铺满且不透明。
    Square { fill: Rgb },
    /// 满幅圆形：Android 的圆形启动图。
    Circle { fill: Rgb },
    /// 透明背景：只有标记本身。
    None,
}

/// 层叠缝隙的处理方式。
///
/// 设计稿的做法是「前一张用底板色描边」，于是缝隙露出底板——
/// 这要求下面必须有底板。透明背景下没有底板可露，只能把缝隙**镂空**。
#[derive(Clone, Copy, Debug)]
pub enum Seam {
    /// 用底板色描边（等价于设计稿的 `stroke=底板色`）。
    Plate(Rgb),
    /// 镂空：用明度遮罩把缝隙擦成透明。
    Erase,
}

/// 一份图标规格。所有交付形态共用同一份几何，只在这里换参数。
#[derive(Clone, Copy, Debug)]
pub struct Spec {
    pub plate: Plate,
    pub mark: Rgb,
    pub seam: Seam,
    /// 标记的组缩放（设计稿默认 1.08）。
    pub mark_scale: f32,
    /// 画布单位的描边宽度（设计稿默认 21.6）。
    pub stroke: f32,
    /// 是否整幅不透明（iOS 的 AppIcon 不允许带 alpha）。
    pub opaque: bool,
}

impl Spec {
    /// 实底（默认交付形态）：强调色超级椭圆 + 白标。
    pub fn solid() -> Self {
        Self {
            plate: Plate::Superellipse {
                fill: ACCENT,
                edge: None,
            },
            mark: WHITE,
            seam: Seam::Plate(ACCENT),
            mark_scale: DESIGN_MARK_SCALE,
            stroke: DESIGN_STROKE,
            opaque: false,
        }
    }

    /// 浅底：淡蓝底板 + 强调色标记 + 细描边。用于浅色桌面或需要低对比度的场景。
    pub fn tinted() -> Self {
        Self {
            plate: Plate::Superellipse {
                fill: TINT_PLATE,
                edge: Some((TINT_EDGE, 4.0)),
            },
            mark: ACCENT,
            // 设计稿的浅底示例里，标记描边同样是底板色。
            seam: Seam::Plate(TINT_PLATE),
            ..Self::solid()
        }
    }

    /// 深底：近黑蓝底板 + 白标。深色模式 / 深色桌面。
    pub fn deep() -> Self {
        let plate = deep_plate();
        Self {
            plate: Plate::Superellipse {
                fill: plate,
                edge: None,
            },
            seam: Seam::Plate(plate),
            ..Self::solid()
        }
    }

    /// 透明背景：只有标记，缝隙镂空。`mark` 取 [`ACCENT`] 或 [`WHITE`]。
    pub fn glyph(mark: Rgb) -> Self {
        Self {
            plate: Plate::None,
            mark,
            seam: Seam::Erase,
            ..Self::solid()
        }
    }

    /// 满幅方形（iOS / PWA maskable）：整幅底色 + 居中标，`ratio` 是标记占宽比例。
    pub fn square(fill: Rgb, mark: Rgb, ratio: f32) -> Self {
        Self {
            plate: Plate::Square { fill },
            mark,
            seam: Seam::Plate(fill),
            mark_scale: scale_for_ratio(ratio),
            stroke: DESIGN_STROKE,
            opaque: true,
        }
    }

    /// 满幅圆形（Android 圆形启动图）。
    pub fn circle(fill: Rgb, mark: Rgb, ratio: f32) -> Self {
        Self {
            plate: Plate::Circle { fill },
            mark,
            seam: Seam::Plate(fill),
            mark_scale: scale_for_ratio(ratio),
            stroke: DESIGN_STROKE,
            opaque: false,
        }
    }

    /// 指定标记占宽比例的变体（自适应图标前景层、maskable 等用）。
    ///
    /// 只改缩放，随后一律经 [`Self::clamp_mark_scale`] 夹取，
    /// 因此不存在「传了个大比例把标记顶出安全区」这种可能。
    pub fn with_mark_ratio(mut self, ratio: f32) -> Self {
        self.mark_scale = scale_for_ratio(ratio);
        self.clamp_mark_scale()
    }

    /// 按目标像素尺寸做小尺寸加固。
    ///
    /// 依据：设计稿要求 16px 仍可辨识，而 21.6 画布单位的描边在 16px 下只有 0.34px，
    /// 缝隙会退化成一条若有若无的灰线。加固到约 0.9 物理像素后，
    /// 两层文件夹的分隔在小尺寸下重新成立。
    ///
    /// 加固后仍要满足设计稿的安全区（标志 ≤ 55% 宽），因此最后统一夹取一次。
    pub fn for_size(mut self, size: u32) -> Self {
        if size <= SMALL_SIZE_MAX {
            self.stroke = self.stroke.max(SMALL_STROKE_PX * CANVAS / size as f32);
            self.mark_scale *= SMALL_MARK_BOOST;
        }
        self.clamp_mark_scale()
    }

    /// 把标记缩放夹进安全区。
    fn clamp_mark_scale(mut self) -> Self {
        let limit = (SAFE_ZONE_RATIO * CANVAS - self.stroke) / MARK_BOX;
        if self.mark_scale > limit {
            self.mark_scale = limit;
        }
        self
    }

    /// 标记（含描边）占画布宽度的比例，用来核对安全区。
    pub fn mark_ratio(&self) -> f32 {
        (MARK_BOX * self.mark_scale + self.stroke) / CANVAS
    }

    /// 生成 SVG 源文本。
    ///
    /// 默认参数下与设计稿的参考 SVG 逐属性一致（同样的路径、同样的组变换、
    /// 同样的 `stroke-width="20"`），差别只在多了 `<title>` 与可选的底板形态。
    pub fn svg(&self, title: &str) -> String {
        let mut svg = String::with_capacity(2048);
        svg.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
        svg.push_str(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 1024 1024\" \
             width=\"1024\" height=\"1024\" role=\"img\"",
        );
        svg.push_str(&format!(" aria-label=\"{title}\">\n"));
        svg.push_str(&format!("  <title>{title}</title>\n"));

        if matches!(self.seam, Seam::Erase) {
            // 透明背景：缝隙必须镂空。明度遮罩里白 = 保留、黑 = 擦除，
            // 于是「先铺满标记色、再按遮罩裁掉缝隙」就得到与实底版完全相同的层次感。
            svg.push_str(
                "  <mask id=\"seam\" maskUnits=\"userSpaceOnUse\" x=\"0\" y=\"0\" \
                 width=\"1024\" height=\"1024\">\n    <rect width=\"1024\" height=\"1024\" fill=\"#000\"/>\n",
            );
            svg.push_str(&self.folder_group("fill=\"#fff\" stroke=\"#000\"", "    "));
            svg.push_str("  </mask>\n");
        }

        match self.plate {
            Plate::Superellipse { fill, edge } => {
                svg.push_str(&format!("  <path d=\"{PLATE_PATH}\" fill=\"{}\"", fill.hex()));
                if let Some((color, width)) = edge {
                    svg.push_str(&format!(
                        " stroke=\"{}\" stroke-width=\"{}\"",
                        color.hex(),
                        num(width)
                    ));
                }
                svg.push_str("/>\n");
            }
            Plate::Square { fill } => svg.push_str(&format!(
                "  <rect width=\"1024\" height=\"1024\" fill=\"{}\"/>\n",
                fill.hex()
            )),
            Plate::Circle { fill } => svg.push_str(&format!(
                "  <circle cx=\"512\" cy=\"512\" r=\"512\" fill=\"{}\"/>\n",
                fill.hex()
            )),
            Plate::None => {}
        }

        match self.seam {
            Seam::Plate(color) => {
                let attrs = format!(
                    "fill=\"{}\" stroke=\"{}\"",
                    self.mark.hex(),
                    color.hex()
                );
                svg.push_str(&self.folder_group(&attrs, "  "));
            }
            Seam::Erase => svg.push_str(&format!(
                "  <rect width=\"1024\" height=\"1024\" fill=\"{}\" mask=\"url(#seam)\"/>\n",
                self.mark.hex()
            )),
        }

        svg.push_str("</svg>\n");
        svg
    }

    /// 两条文件夹路径 + 组变换。
    ///
    /// `stroke-width` 写在组内，因此是**局部单位**：要得到画布上 `stroke` 的宽度，
    /// 必须除以组缩放。默认参数下这一步恰好抵消回设计稿的 `stroke-width="20"`。
    fn folder_group(&self, attrs: &str, indent: &str) -> String {
        let scale = num(self.mark_scale);
        let stroke = num(self.stroke / self.mark_scale);
        format!(
            "{indent}<g transform=\"translate(512 512) scale({scale}) translate(-512 -512)\" \
             stroke-width=\"{stroke}\" {attrs}>\n\
             {indent}  <path d=\"{FOLDER_BACK}\"/>\n\
             {indent}  <path d=\"{FOLDER_FRONT}\"/>\n\
             {indent}</g>\n"
        )
    }
}

/// 由「标记占宽比例」反推组缩放。
fn scale_for_ratio(ratio: f32) -> f32 {
    (ratio * CANVAS - DESIGN_STROKE) / MARK_BOX
}

/// 标记局部坐标 → 画布坐标。
///
/// 校验脚本据此把设计稿里的局部坐标（例如前层文件夹的左边缘 x = 366）
/// 换算成探针位置，避免把「1.08 倍缩放之后到底落在哪」写死成魔数。
pub fn to_canvas(mark_coordinate: f32, scale: f32) -> f32 {
    CENTER + (mark_coordinate - CENTER) * scale
}

/// 数字格式化：最多 4 位小数并去掉尾随零，避免 SVG 里出现 `1.0800` 这种噪声。
fn num(value: f32) -> String {
    let text = format!("{value:.4}");
    let trimmed = text.trim_end_matches('0').trim_end_matches('.');
    if trimmed.is_empty() || trimmed == "-" {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accent_matches_the_published_conversion() {
        // 设计稿写明 oklch(55% 0.2 258) 的 sRGB 换算是 #076be3。
        assert_eq!(oklch_to_rgb(0.55, 0.2, 258.0).hex(), "#076be3");
    }

    #[test]
    fn deep_plate_is_a_near_black_blue() {
        let plate = deep_plate();
        // 近黑蓝：蓝色分量最大，整体仍然很暗。
        assert!(
            plate.2 > plate.1 && plate.1 > plate.0,
            "深底应偏蓝：{plate:?}"
        );
        assert!(plate.2 < 0x60, "深底应足够暗：{plate:?}");
    }

    #[test]
    fn default_spec_keeps_the_design_stroke_on_the_svg() {
        let svg = Spec::solid().svg("t");
        // 默认参数下必须原样写出设计稿的 20（组内单位）。
        assert!(svg.contains("stroke-width=\"20\""), "{svg}");
        assert!(svg.contains("scale(1.08)"), "{svg}");
        assert!(svg.contains(ACCENT.hex().as_str()), "{svg}");
    }

    #[test]
    fn every_variant_stays_inside_the_safe_zone() {
        let variants = [
            Spec::solid(),
            Spec::tinted(),
            Spec::deep(),
            Spec::glyph(ACCENT),
            Spec::glyph(WHITE),
            Spec::square(ACCENT, WHITE, 0.52),
            Spec::circle(ACCENT, WHITE, 0.52),
        ];
        for variant in variants {
            for size in [16, 20, 24, 29, 32, 48, 64, 128, 256, 512, 1024] {
                let spec = variant.for_size(size);
                assert!(
                    spec.mark_ratio() <= SAFE_ZONE_RATIO + 1e-3,
                    "尺寸 {size} 下标记占宽 {:.3} 超出安全区",
                    spec.mark_ratio()
                );
            }
        }
    }

    #[test]
    fn small_sizes_get_a_thicker_seam() {
        let small = Spec::solid().for_size(16);
        let large = Spec::solid().for_size(256);
        assert!(small.stroke > large.stroke);
        // 16px 下描边应接近 0.9 物理像素。
        let pixels = small.stroke * 16.0 / CANVAS;
        assert!((pixels - SMALL_STROKE_PX).abs() < 0.01, "实际 {pixels}px");
        // 大尺寸不受影响，保持设计稿取值。
        assert!((large.stroke - DESIGN_STROKE).abs() < f32::EPSILON);
        assert!((large.mark_scale - DESIGN_MARK_SCALE).abs() < f32::EPSILON);
    }

    #[test]
    fn glyph_variant_uses_a_mask_instead_of_a_plate_coloured_stroke() {
        let svg = Spec::glyph(ACCENT).svg("t");
        assert!(svg.contains("<mask id=\"seam\""), "{svg}");
        assert!(svg.contains("mask=\"url(#seam)\""), "{svg}");
        // 透明背景不该出现任何底板几何。
        assert!(!svg.contains(PLATE_PATH), "{svg}");
    }

    #[test]
    fn square_variant_is_full_bleed_and_opaque() {
        let spec = Spec::square(ACCENT, WHITE, 0.52);
        let svg = spec.svg("t");
        assert!(svg.contains("<rect width=\"1024\" height=\"1024\""));
        assert!(spec.opaque);
        // 满幅方形不该有圆角底板。
        assert!(!svg.contains(PLATE_PATH));
    }

    #[test]
    fn canvas_conversion_matches_the_group_transform() {
        // 组变换 scale(1.08) 绕画布中心：局部 302 → 285.2，局部 722 → 738.8。
        assert!((to_canvas(302.0, 1.08) - 285.2).abs() < 0.01);
        assert!((to_canvas(722.0, 1.08) - 738.8).abs() < 0.01);
        // 中心是不动点。
        assert!((to_canvas(512.0, 1.08) - 512.0).abs() < 1e-3);
    }

    #[test]
    fn numbers_are_formatted_without_trailing_noise() {
        assert_eq!(num(20.0), "20");
        assert_eq!(num(1.08), "1.08");
        assert_eq!(num(0.0), "0");
    }
}
