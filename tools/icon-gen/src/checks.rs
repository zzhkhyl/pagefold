//! 生成后的自检。
//!
//! 每一项都对应一条**设计稿里的硬约束或平台要求**，而不是「看起来差不多」：
//! 安全区比例、层叠缝隙是否真的存在、透明版是否真的镂空、iOS 是否真的没有 alpha 通道、
//! 二进制容器结构是否自洽。任何一项失败都视为产物不可交付（`main` 会以非零码退出）。
//!
//! 探针坐标由设计稿的常量推算（[`design::to_canvas`]），不是写死的魔数——
//! 这样改了标记缩放之后，探针会跟着走到正确的位置。

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use resvg::tiny_skia::Pixmap;

use crate::design::{self, ACCENT, SAFE_ZONE_RATIO, SMALL_STROKE_PX, Spec, WHITE};
use crate::raster::{self, Vector};
use crate::{
    ANDROID_DENSITIES, ATLAS, BLEED_MARK_RATIO, FAVICON_SIZES, ICO_SIZES, IOS_FILES, OUT_DIR,
    SAFE_CIRCLE_MARK_RATIO,
};

/// 一条校验结果。
pub struct Check {
    pub label: String,
    pub passed: bool,
    pub detail: String,
}

impl Check {
    fn new(label: impl Into<String>, passed: bool, detail: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            passed,
            detail: detail.into(),
        }
    }
}

/// 跑完全部校验。
pub fn run(out: &Path) -> Result<Vec<Check>> {
    let checks = vec![
        check_transparent_corners(out)?,
        check_center_is_white(out)?,
        check_seam_exists(out)?,
        check_glyph_seam_is_erased(out)?,
        check_bleed_variants(out)?,
        check_safe_zone()?,
        check_small_size_legibility(out)?,
        check_small_size_boost()?,
        check_ios_has_no_alpha(out)?,
        check_ico_structure(out)?,
        check_icns_structure(out)?,
        check_png_headers(out)?,
        check_variant_colours(out)?,
        check_file_list(out)?,
    ];
    Ok(checks)
}

/// 超级椭圆底板不覆盖画布四角，因此四角必须是透明的。
fn check_transparent_corners(_out: &Path) -> Result<Check> {
    let probe = Probe::new(&Spec::solid(), 1024)?;
    let corners = [
        probe.alpha(3, 3),
        probe.alpha(1020, 3),
        probe.alpha(3, 1020),
        probe.alpha(1020, 1020),
    ];
    let passed = corners.iter().all(|alpha| *alpha == 0);
    Ok(Check::new(
        "实底：四角透明（超级椭圆底板不留方角）",
        passed,
        format!("四角 alpha = {corners:?}"),
    ))
}

/// 画布中心落在前层文件夹内部，必须是纯白。
fn check_center_is_white(_out: &Path) -> Result<Check> {
    let probe = Probe::new(&Spec::solid(), 1024)?;
    let pixel = probe.pixel(512, 512);
    let passed = near(pixel, [255, 255, 255, 255], 2);
    Ok(Check::new(
        "实底：中心为白（前层文件夹覆盖画布中心）",
        passed,
        format!("(512,512) = {pixel:?}"),
    ))
}

/// 层叠效果的全部价值就在这条缝隙上：它必须真的存在，且是底板色。
fn check_seam_exists(_out: &Path) -> Result<Check> {
    let spec = Spec::solid();
    let scale = spec.for_size(1024).mark_scale;
    // 前层文件夹的左边缘（设计稿局部坐标 x = 366）换算到画布坐标，
    // 该处被前层的底板色描边覆盖，因此应当仍是强调色而不是白。
    let seam_x = design::to_canvas(366.0, scale).round() as u32;
    let probe = Probe::new(&spec, 1024)?;
    let pixel = probe.pixel(seam_x, 512);
    let passed = near(pixel, to_rgba(ACCENT), 20);
    Ok(Check::new(
        "实底：层叠缝隙存在且为底板色",
        passed,
        format!("(x={seam_x}, y=512) = {pixel:?}，期望接近 {}", ACCENT.hex()),
    ))
}

/// 透明背景下没有底板可露，缝隙只能镂空。
fn check_glyph_seam_is_erased(_out: &Path) -> Result<Check> {
    let spec = Spec::glyph(WHITE);
    let scale = spec.for_size(1024).mark_scale;
    let seam_x = design::to_canvas(366.0, scale).round() as u32;
    let probe = Probe::new(&spec, 1024)?;
    let seam = probe.alpha(seam_x, 512);
    let inside = probe.alpha(512, 512);
    let corner = probe.alpha(3, 3);
    let passed = seam < 64 && inside == 255 && corner == 0;
    Ok(Check::new(
        "透明背景：缝隙已镂空（遮罩生效）",
        passed,
        format!("缝 alpha={seam}（应<64），标记内 alpha={inside}（应=255），角 alpha={corner}（应=0）"),
    ))
}

/// 满幅方形 / 圆形的几何要求：方形铺满且不透明，圆形四角透明。
fn check_bleed_variants(_out: &Path) -> Result<Check> {
    let square = Probe::new(&Spec::square(ACCENT, WHITE, BLEED_MARK_RATIO), 256)?;
    let opaque = square.is_fully_opaque();
    let corner = square.pixel(1, 1);
    let square_ok = opaque && near(corner, to_rgba(ACCENT), 2);

    let circle = Probe::new(&Spec::circle(ACCENT, WHITE, BLEED_MARK_RATIO), 256)?;
    let circle_ok = circle.alpha(1, 1) == 0 && circle.alpha(128, 128) == 255;

    Ok(Check::new(
        "满幅方形铺满且不透明；满幅圆形四角透明",
        square_ok && circle_ok,
        format!("方形角 {corner:?} 全不透明={opaque}；圆形角 alpha={} 中心 alpha={}",
            circle.alpha(1, 1), circle.alpha(128, 128)),
    ))
}

/// 设计稿规定标志宽度不超过画布的 55%，对**所有**「规格 × 尺寸」组合逐一核对。
fn check_safe_zone() -> Result<Check> {
    let specs = [
        ("实底", Spec::solid()),
        ("浅底", Spec::tinted()),
        ("深底", Spec::deep()),
        ("透明·蓝标", Spec::glyph(ACCENT)),
        ("透明·白标", Spec::glyph(WHITE)),
        ("满幅方形", Spec::square(ACCENT, WHITE, BLEED_MARK_RATIO)),
        ("满幅圆形", Spec::circle(ACCENT, WHITE, BLEED_MARK_RATIO)),
        ("圆形启动图", Spec::circle(ACCENT, WHITE, BLEED_MARK_RATIO)),
        (
            "自适应前景",
            Spec::glyph(WHITE).with_mark_ratio(SAFE_CIRCLE_MARK_RATIO),
        ),
        (
            "maskable",
            Spec::square(ACCENT, WHITE, SAFE_CIRCLE_MARK_RATIO),
        ),
    ];

    let mut sizes: Vec<u32> = ATLAS.to_vec();
    sizes.extend_from_slice(&ICO_SIZES);
    sizes.extend_from_slice(&FAVICON_SIZES);
    sizes.extend(IOS_FILES.iter().map(|(_, size)| *size));
    sizes.extend(ANDROID_DENSITIES.iter().flat_map(|(_, a, b)| [*a, *b]));
    sizes.extend([180, 192, 512]);
    sizes.sort_unstable();
    sizes.dedup();

    let mut worst = (String::from("—"), 0.0f32, 0u32);
    for (name, spec) in specs {
        for size in &sizes {
            let ratio = spec.for_size(*size).mark_ratio();
            if ratio > worst.1 {
                worst = (name.to_string(), ratio, *size);
            }
        }
    }

    Ok(Check::new(
        "安全区：所有规格 × 尺寸的标记占宽 ≤ 55%",
        worst.1 <= SAFE_ZONE_RATIO + 1e-3,
        format!(
            "共 {} 个组合，最大 {:.1}%（{} @ {}px）",
            specs.len() * sizes.len(),
            worst.1 * 100.0,
            worst.0,
            worst.2
        ),
    ))
}

/// 16px 下白色标记必须仍然占够像素——这是「小尺寸仍可辨识」的量化口径。
fn check_small_size_legibility(_out: &Path) -> Result<Check> {
    let probe = Probe::new(&Spec::solid(), 16)?;
    let white = probe.count(|pixel| pixel[3] > 200 && pixel[0] > 200 && pixel[1] > 200 && pixel[2] > 200);
    let plate = probe.count(|pixel| pixel[3] > 200 && !(pixel[0] > 200 && pixel[1] > 200 && pixel[2] > 200));
    // 10 像素是「能看出是个白色形状」的下限；上限用来挡住「糊成一团白」。
    let passed = (10..=200).contains(&white) && plate > 40;
    Ok(Check::new(
        "16px：白色标记仍有可辨识的面积",
        passed,
        format!("16×16=256 像素中，标记 {white} 像素、底板 {plate} 像素"),
    ))
}

/// 小尺寸加固必须真的生效（否则 16px 的缝隙会退化成灰线）。
fn check_small_size_boost() -> Result<Check> {
    let small = Spec::solid().for_size(16);
    let large = Spec::solid().for_size(256);
    let pixels = small.stroke * 16.0 / design::CANVAS;
    let passed = pixels >= SMALL_STROKE_PX - 0.05
        && (large.stroke - design::DESIGN_STROKE).abs() < f32::EPSILON;
    Ok(Check::new(
        "小尺寸加固：≤32px 加粗描边、≥48px 保持设计稿取值",
        passed,
        format!(
            "16px 描边 {pixels:.2}px（目标 {SMALL_STROKE_PX}px）；256px 描边 {:.1} 画布单位",
            large.stroke
        ),
    ))
}

/// iOS 的 AppIcon 不允许带 alpha 通道：逐个读回产物确认颜色类型是 RGB。
fn check_ios_has_no_alpha(out: &Path) -> Result<Check> {
    let mut failures = Vec::new();
    for (suffix, size) in IOS_FILES {
        let path = out.join("ios").join(format!("AppIcon-{suffix}.png"));
        let bytes = fs::read(&path).with_context(|| format!("读取失败：{}", path.display()))?;
        let (width, height, color) = png_header(&bytes)?;
        if color != 2 || width != size || height != size {
            failures.push(format!(
                "AppIcon-{suffix}.png = {width}×{height} colorType {color}"
            ));
        }
    }
    Ok(Check::new(
        "iOS：全部 AppIcon 为 RGB（无 alpha 通道）且尺寸正确",
        failures.is_empty(),
        if failures.is_empty() {
            format!("{} 个文件全部为 color type 2", IOS_FILES.len())
        } else {
            failures.join("；")
        },
    ))
}

/// ICO 目录必须自洽：条目数、偏移、长度、以及 BMP 载荷的「高度 ×2」约定。
fn check_ico_structure(out: &Path) -> Result<Check> {
    let path = out.join("windows").join("icon.ico");
    let bytes = fs::read(&path).with_context(|| format!("读取失败：{}", path.display()))?;
    if bytes.len() < 6 {
        bail!("ICO 文件过短");
    }
    let count = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
    let mut problems = Vec::new();
    if count != ICO_SIZES.len() {
        problems.push(format!("条目数 {count}，期望 {}", ICO_SIZES.len()));
    }

    for index in 0..count {
        let base = 6 + index * 16;
        let size = if bytes[base] == 0 {
            256
        } else {
            bytes[base] as u32
        };
        let length =
            u32::from_le_bytes([bytes[base + 8], bytes[base + 9], bytes[base + 10], bytes[base + 11]])
                as usize;
        let offset = u32::from_le_bytes([
            bytes[base + 12],
            bytes[base + 13],
            bytes[base + 14],
            bytes[base + 15],
        ]) as usize;
        if offset + length > bytes.len() {
            problems.push(format!("第 {index} 项越界"));
            continue;
        }
        // ≤64 的条目是 BMP：头里的高度字段是实际高度的两倍。
        if size <= 64 {
            let payload = &bytes[offset..offset + length];
            let height = i32::from_le_bytes([payload[8], payload[9], payload[10], payload[11]]);
            if height != (size * 2) as i32 {
                problems.push(format!("{size}px 的 BMP 高度字段为 {height}"));
            }
        }
    }

    Ok(Check::new(
        "ICO 目录与载荷自洽",
        problems.is_empty(),
        if problems.is_empty() {
            format!("{count} 个尺寸：{ICO_SIZES:?}，文件 {} B", bytes.len())
        } else {
            problems.join("；")
        },
    ))
}

/// ICNS 的结构：魔数、总长度、每个数据块的长度都必须与文件长度一致。
fn check_icns_structure(out: &Path) -> Result<Check> {
    let path = out.join("macos").join("icon.icns");
    let bytes = fs::read(&path).with_context(|| format!("读取失败：{}", path.display()))?;
    let mut problems = Vec::new();
    if &bytes[0..4] != b"icns" {
        problems.push("魔数不是 icns".to_string());
    }
    let declared = u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
    if declared != bytes.len() {
        problems.push(format!("声明长度 {declared}，实际 {}", bytes.len()));
    }

    let mut kinds = Vec::new();
    let mut cursor = 8usize;
    while cursor + 8 <= bytes.len() {
        let kind = String::from_utf8_lossy(&bytes[cursor..cursor + 4]).to_string();
        let length =
            u32::from_be_bytes([bytes[cursor + 4], bytes[cursor + 5], bytes[cursor + 6], bytes[cursor + 7]])
                as usize;
        if length < 8 || cursor + length > bytes.len() {
            problems.push(format!("数据块 {kind} 长度 {length} 越界"));
            break;
        }
        kinds.push(kind);
        cursor += length;
    }
    if cursor != bytes.len() {
        problems.push(format!("数据块总长 {cursor} 与文件长度 {} 不一致", bytes.len()));
    }

    Ok(Check::new(
        "ICNS 数据块自洽",
        problems.is_empty(),
        if problems.is_empty() {
            format!("{} 个数据块：{}", kinds.len(), kinds.join(" "))
        } else {
            problems.join("；")
        },
    ))
}

/// 主图集的 PNG 头必须与文件名声明的尺寸一致。
fn check_png_headers(out: &Path) -> Result<Check> {
    let mut problems = Vec::new();
    for size in ATLAS {
        let path = out
            .join("png")
            .join(format!("icon-c-folder-{size}.png"));
        let bytes = fs::read(&path).with_context(|| format!("读取失败：{}", path.display()))?;
        let (width, height, color) = png_header(&bytes)?;
        if width != size || height != size {
            problems.push(format!("{size}.png = {width}×{height}"));
        }
        if color != 6 {
            problems.push(format!("{size}.png 颜色类型 {color}（期望 6 = RGBA）"));
        }
    }
    Ok(Check::new(
        "主图集：PNG 尺寸与文件名一致且带 alpha 通道",
        problems.is_empty(),
        if problems.is_empty() {
            format!("{} 个尺寸：{ATLAS:?}", ATLAS.len())
        } else {
            problems.join("；")
        },
    ))
}

/// 深底 / 浅底的色值必须来自设计稿的令牌，而不是手抄的近似值。
fn check_variant_colours(out: &Path) -> Result<Check> {
    let deep = fs::read_to_string(out.join("src").join("icon-c-folder-deep.svg"))?;
    let tinted = fs::read_to_string(out.join("src").join("icon-c-folder-tinted.svg"))?;
    let expected_deep = design::deep_plate().hex();

    let mut problems = Vec::new();
    if !deep.contains(&expected_deep) {
        problems.push(format!("深底缺少 {expected_deep}"));
    }
    for token in [design::TINT_PLATE.hex(), design::TINT_EDGE.hex(), ACCENT.hex()] {
        if !tinted.contains(&token) {
            problems.push(format!("浅底缺少 {token}"));
        }
    }

    Ok(Check::new(
        "深底 / 浅底色值来自设计稿令牌",
        problems.is_empty(),
        if problems.is_empty() {
            format!("深底 {expected_deep}；浅底 {} + {} + {}", design::TINT_PLATE.hex(), design::TINT_EDGE.hex(), ACCENT.hex())
        } else {
            problems.join("；")
        },
    ))
}

/// 交付清单必须齐全——少一个文件比多一个文件更难被发现。
fn check_file_list(out: &Path) -> Result<Check> {
    let mut expected: Vec<String> = Vec::new();

    for name in [
        "icon-c-folder.svg",
        "icon-c-folder-tinted.svg",
        "icon-c-folder-deep.svg",
        "icon-c-folder-glyph.svg",
        "icon-c-folder-glyph-white.svg",
        "icon-c-folder-square.svg",
        "icon-c-folder-circle.svg",
    ] {
        expected.push(format!("src/{name}"));
    }
    for size in ATLAS {
        expected.push(format!("png/icon-c-folder-{size}.png"));
    }
    expected.push("windows/icon.ico".into());
    expected.push("windows/icon.rc".into());
    expected.push("macos/icon.icns".into());
    for (density, _, _) in ANDROID_DENSITIES {
        for name in [
            "ic_launcher.png",
            "ic_launcher_round.png",
            "ic_launcher_foreground.png",
        ] {
            expected.push(format!("android/mipmap-{density}/{name}"));
        }
    }
    expected.push("android/values/ic_launcher_background.xml".into());
    expected.push("android/mipmap-anydpi-v26/ic_launcher.xml".into());
    expected.push("android/mipmap-anydpi-v26/ic_launcher_round.xml".into());
    for (suffix, _) in IOS_FILES {
        expected.push(format!("ios/AppIcon-{suffix}.png"));
    }
    expected.push("ios/Contents.json".into());
    for name in [
        "favicon-16.png",
        "favicon-32.png",
        "favicon-48.png",
        "favicon.ico",
        "apple-touch-icon-180.png",
        "icon-192.png",
        "icon-512.png",
        "icon-maskable-512.png",
        "icon.svg",
        "site.webmanifest",
    ] {
        expected.push(format!("web/{name}"));
    }
    for (group, name) in [
        ("dark", "icon-c-folder-deep"),
        ("light", "icon-c-folder-tinted"),
        ("transparent", "icon-c-folder-glyph"),
        ("transparent", "icon-c-folder-glyph-white"),
    ] {
        for size in [256u32, 512, 1024] {
            expected.push(format!("variants/{group}/{name}-{size}.png"));
        }
    }

    let missing: Vec<String> = expected
        .iter()
        .filter(|relative| !out.join(relative.as_str()).is_file())
        .cloned()
        .collect();

    Ok(Check::new(
        "交付清单完整",
        missing.is_empty(),
        if missing.is_empty() {
            format!("{} 个文件全部存在（根目录 {}）", expected.len(), OUT_DIR)
        } else {
            format!("缺失 {} 个：{}", missing.len(), missing.join(" "))
        },
    ))
}

/// 渲染结果的内存探针。
struct Probe {
    pixmap: Pixmap,
}

impl Probe {
    fn new(spec: &Spec, size: u32) -> Result<Self> {
        let adjusted = spec.for_size(size);
        let vector = Vector::parse(&adjusted.svg("校验"))?;
        Ok(Self {
            pixmap: vector.render(size)?,
        })
    }

    fn alpha(&self, x: u32, y: u32) -> u8 {
        raster::alpha(&self.pixmap, x, y)
    }

    fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        raster::pixel(&self.pixmap, x, y)
    }

    fn is_fully_opaque(&self) -> bool {
        raster::is_fully_opaque(&self.pixmap)
    }

    /// 统计满足条件的像素数。
    fn count(&self, predicate: impl Fn([u8; 4]) -> bool) -> usize {
        let (width, height) = (self.pixmap.width(), self.pixmap.height());
        let mut total = 0;
        for y in 0..height {
            for x in 0..width {
                if predicate(self.pixel(x, y)) {
                    total += 1;
                }
            }
        }
        total
    }
}

/// 读 PNG 的 IHDR：宽、高、颜色类型。
fn png_header(bytes: &[u8]) -> Result<(u32, u32, u8)> {
    if bytes.len() < 26 || &bytes[0..8] != b"\x89PNG\r\n\x1a\n" {
        bail!("不是合法的 PNG");
    }
    let width = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let height = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    Ok((width, height, bytes[25]))
}

/// 两个颜色是否在容差内相等（抗锯齿会让边界像素略有偏差）。
fn near(actual: [u8; 4], expected: [u8; 4], tolerance: u8) -> bool {
    actual
        .iter()
        .zip(expected.iter())
        .all(|(a, b)| a.abs_diff(*b) <= tolerance)
}

fn to_rgba(color: design::Rgb) -> [u8; 4] {
    [color.0, color.1, color.2, 255]
}
