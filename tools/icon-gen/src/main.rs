//! 由 `design/app-icons.html` 的 **C 方案（层叠文件夹）** 生成全平台应用图标。
//!
//! 运行：
//! ```text
//! cargo run --release --manifest-path tools/icon-gen/Cargo.toml
//! ```
//! 产物全部落在 `assets/app-icon/`（说明文档 `assets/app-icon/README.md` 由人工维护，
//! 本工具只负责画得出、画得准、可复现）。
//!
//! 设计取舍写在这里，而不是散在各处：
//!
//! 1. **一份几何，参数化变体**。底板形态、缝隙处理、标记缩放、描边宽度都是
//!    [`design::Spec`] 的字段，因此实底 / 浅底 / 深底 / 透明 / 满幅方形 / 圆形之间
//!    不可能出现几何漂移。
//! 2. **每个尺寸独立栅格化**，而不是把 1024 缩小——小尺寸是在目标分辨率下重新求一遍几何。
//! 3. **小尺寸加固**（≤32px）：把缝隙描边加粗到约 0.9 物理像素并放大标记 8%，
//!    并强制满足设计稿的 55% 安全区（见 `design::Spec::for_size`）。
//! 4. **iOS 单独走 RGB PNG**（无 alpha 通道），因为 AppIcon 不允许带透明度。

mod checks;
mod containers;
mod design;
mod raster;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use containers::{IcnsChunk, IcoImage};
use design::{ACCENT, Spec, WHITE};
use raster::Vector;

/// 交付根目录（相对仓库根）。
const OUT_DIR: &str = "assets/app-icon";

/// 主图集的尺寸：需求点名的 8 档，外加 Windows 常用的 24。
const ATLAS: [u32; 9] = [16, 24, 32, 48, 64, 128, 256, 512, 1024];

/// `.ico` 里包含的尺寸。≤64 用 BMP、≥128 用 PNG。
const ICO_SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];

/// 浏览器 `favicon.ico` 的尺寸。
const FAVICON_SIZES: [u32; 3] = [16, 32, 48];

/// 满幅形态下标记占画布宽度的比例。
///
/// 0.52 而不是设计稿实底版的 0.464：满幅方形 / 圆形没有底板留白，
/// 标记需要更大才压得住整块色；同时仍低于设计稿 55% 的安全区上限。
const BLEED_MARK_RATIO: f32 = 0.52;

/// 自适应图标（Android 前景层 / PWA maskable）的标记比例。
///
/// 这两个场景的内容会被系统裁成圆形，安全圆直径约占画布 61%，
/// 而标记包围盒是正方形（对角线是边长的 1.41 倍），
/// 因此比例必须压到 `0.611 / 1.41 ≈ 0.43` 以内——取 0.42。
const SAFE_CIRCLE_MARK_RATIO: f32 = 0.42;

/// iOS 图标文件：文件名后缀 → 像素尺寸。
const IOS_FILES: [(&str, u32); 15] = [
    ("20@1x", 20),
    ("20@2x", 40),
    ("20@3x", 60),
    ("29@1x", 29),
    ("29@2x", 58),
    ("29@3x", 87),
    ("40@1x", 40),
    ("40@2x", 80),
    ("40@3x", 120),
    ("60@2x", 120),
    ("60@3x", 180),
    ("76@1x", 76),
    ("76@2x", 152),
    ("83.5@2x", 167),
    ("1024@1x", 1024),
];

/// `Contents.json` 的条目：`(idiom, size, scale, 文件后缀)`。
///
/// 20@2x 同时属于 iphone 与 ipad，两个条目指向同一个文件——这是 Asset Catalog 允许的，
/// 也正因如此它才需要显式列两遍，而不是靠去重「猜」出来。
const IOS_ENTRIES: [(&str, &str, &str, &str); 16] = [
    ("iphone", "20x20", "2x", "20@2x"),
    ("iphone", "20x20", "3x", "20@3x"),
    ("iphone", "29x29", "2x", "29@2x"),
    ("iphone", "29x29", "3x", "29@3x"),
    ("iphone", "40x40", "2x", "40@2x"),
    ("iphone", "40x40", "3x", "40@3x"),
    ("iphone", "60x60", "2x", "60@2x"),
    ("iphone", "60x60", "3x", "60@3x"),
    ("ipad", "20x20", "1x", "20@1x"),
    ("ipad", "20x20", "2x", "20@2x"),
    ("ipad", "29x29", "1x", "29@1x"),
    ("ipad", "40x40", "1x", "40@1x"),
    ("ipad", "76x76", "1x", "76@1x"),
    ("ipad", "76x76", "2x", "76@2x"),
    ("ipad", "83.5x83.5", "2x", "83.5@2x"),
    ("ios-marketing", "1024x1024", "1x", "1024@1x"),
];

/// Android 密度目录 → `(启动图边长, 自适应前景层边长)`。
///
/// 前景层是 108dp 画布（mdpi 108px、xxxhdpi 432px），
/// 系统只保证中央 66dp 圆内可见，因此标记比例另用 [`SAFE_CIRCLE_MARK_RATIO`]。
const ANDROID_DENSITIES: [(&str, u32, u32); 5] = [
    ("mdpi", 48, 108),
    ("hdpi", 72, 162),
    ("xhdpi", 96, 216),
    ("xxhdpi", 144, 324),
    ("xxxhdpi", 192, 432),
];

fn main() -> Result<()> {
    let root = repo_root()?;
    let out = root.join(OUT_DIR);

    let mut session = Session::new(root.clone());

    emit_sources(&out, &mut session)?;
    emit_atlas(&out, &mut session)?;
    emit_windows(&out, &mut session)?;
    emit_macos(&out, &mut session)?;
    emit_android(&out, &mut session)?;
    emit_ios(&out, &mut session)?;
    emit_web(&out, &mut session)?;
    emit_variants(&out, &mut session)?;

    let checks = checks::run(&out)?;
    session.report(&checks);

    if checks.iter().any(|check| !check.passed) {
        bail!("有校验项未通过，产物不可交付");
    }
    Ok(())
}

/// 1) 矢量源：唯一真源，七种形态各一份。
fn emit_sources(out: &Path, session: &mut Session) -> Result<()> {
    let sources: [(&str, Spec, &str); 7] = [
        ("icon-c-folder.svg", Spec::solid(), "叠页 · 层叠文件夹 · 实底"),
        (
            "icon-c-folder-tinted.svg",
            Spec::tinted(),
            "叠页 · 层叠文件夹 · 浅底",
        ),
        (
            "icon-c-folder-deep.svg",
            Spec::deep(),
            "叠页 · 层叠文件夹 · 深底",
        ),
        (
            "icon-c-folder-glyph.svg",
            Spec::glyph(ACCENT),
            "叠页 · 层叠文件夹 · 透明背景 · 蓝标",
        ),
        (
            "icon-c-folder-glyph-white.svg",
            Spec::glyph(WHITE),
            "叠页 · 层叠文件夹 · 透明背景 · 白标",
        ),
        (
            "icon-c-folder-square.svg",
            Spec::square(ACCENT, WHITE, BLEED_MARK_RATIO),
            "叠页 · 层叠文件夹 · 满幅方形",
        ),
        (
            "icon-c-folder-circle.svg",
            Spec::circle(ACCENT, WHITE, BLEED_MARK_RATIO),
            "叠页 · 层叠文件夹 · 满幅圆形",
        ),
    ];

    for (name, spec, title) in sources {
        session.write("矢量源", &out.join("src").join(name), spec.svg(title).as_bytes())?;
    }
    Ok(())
}

/// 2) 主图集：实底，逐尺寸独立栅格化。
fn emit_atlas(out: &Path, session: &mut Session) -> Result<()> {
    for size in ATLAS {
        let bytes = render(&Spec::solid(), size)?;
        session.write(
            "主图集 PNG",
            &out.join("png").join(format!("icon-c-folder-{size}.png")),
            &bytes,
        )?;
    }
    Ok(())
}

/// 3) Windows：`.ico` + 资源脚本。
fn emit_windows(out: &Path, session: &mut Session) -> Result<()> {
    let ico = build_ico(&Spec::solid(), &ICO_SIZES)?;
    session.write("Windows", &out.join("windows").join("icon.ico"), &ico)?;

    // 资源脚本默认不参与构建：接入需要 build.rs 或 winres，
    // 属于产品侧的取舍，这里只把可直接使用的文件附上（见 README）。
    let rc = "\
// 叠页 · 应用图标资源脚本
//
// 用法（二选一）：
//   1) 命令行：rc.exe /fo icon.res windows/icon.rc 后随其它 .res 一起交给链接器；
//   2) build.rs：用 `winresource` 或 `embed-resource` 指定本文件。
//
// 注意：本文件默认**未接入** `fm-app` 的构建（见 assets/app-icon/README.md 的说明）。
IDI_APPICON ICON \"icon.ico\"
";
    session.write("Windows", &out.join("windows").join("icon.rc"), rc.as_bytes())?;
    Ok(())
}

/// 4) macOS：`.icns`。
///
/// 数据块覆盖 16/32/64/128/256/512/1024，以及 16@2x、32@2x、128@2x、256@2x 这几个 @2x 槽位。
fn emit_macos(out: &Path, session: &mut Session) -> Result<()> {
    let spec = Spec::solid();
    let layout: [(&[u8; 4], u32); 11] = [
        (b"icp4", 16),
        (b"icp5", 32),
        (b"icp6", 64),
        (b"ic07", 128),
        (b"ic08", 256),
        (b"ic09", 512),
        (b"ic10", 1024),
        (b"ic11", 32),
        (b"ic12", 64),
        (b"ic13", 256),
        (b"ic14", 512),
    ];

    let mut chunks = Vec::with_capacity(layout.len());
    for (kind, size) in layout {
        chunks.push(IcnsChunk {
            kind: *kind,
            payload: render(&spec, size)?,
        });
    }
    let icns = containers::write_icns(&chunks)?;
    session.write("macOS", &out.join("macos").join("icon.icns"), &icns)?;
    Ok(())
}

/// 5) Android：传统启动图 + 圆形启动图 + 自适应图标的前景/背景层。
fn emit_android(out: &Path, session: &mut Session) -> Result<()> {
    let android = out.join("android");
    let round = Spec::circle(ACCENT, WHITE, BLEED_MARK_RATIO);
    let foreground = Spec::glyph(WHITE).with_mark_ratio(SAFE_CIRCLE_MARK_RATIO);

    for (density, launcher, adaptive) in ANDROID_DENSITIES {
        let dir = android.join(format!("mipmap-{density}"));
        session.write(
            "Android",
            &dir.join("ic_launcher.png"),
            &render(&Spec::solid(), launcher)?,
        )?;
        session.write(
            "Android",
            &dir.join("ic_launcher_round.png"),
            &render(&round, launcher)?,
        )?;
        session.write(
            "Android",
            &dir.join("ic_launcher_foreground.png"),
            &render(&foreground, adaptive)?,
        )?;
    }

    // 自适应图标的背景层用颜色资源，而不是一张铺满的位图：
    // 这样系统做视差 / 缩放时不会放大出锯齿。
    let background = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<resources>\n    <!-- 叠页 · 层叠文件夹 · 实底底板色 oklch(55% 0.2 258) -->\n    <color name=\"ic_launcher_background\">{}</color>\n</resources>\n",
        ACCENT.hex()
    );
    session.write(
        "Android",
        &android.join("values").join("ic_launcher_background.xml"),
        background.as_bytes(),
    )?;

    // 自适应图标声明（API 26+）。圆形与传统两种都在此声明，
    // 系统按启动器形状自行裁切。
    let adaptive = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
        <adaptive-icon xmlns:android=\"http://schemas.android.com/apk/res/android\">\n\
        \x20   <background android:drawable=\"@color/ic_launcher_background\"/>\n\
        \x20   <foreground android:drawable=\"@mipmap/ic_launcher_foreground\"/>\n\
        </adaptive-icon>\n";
    for name in ["ic_launcher.xml", "ic_launcher_round.xml"] {
        session.write(
            "Android",
            &android.join("mipmap-anydpi-v26").join(name),
            adaptive.as_bytes(),
        )?;
    }
    Ok(())
}

/// 6) iOS：满幅方形、无 alpha 通道，并附 `Contents.json`。
fn emit_ios(out: &Path, session: &mut Session) -> Result<()> {
    let ios = out.join("ios");
    let spec = Spec::square(ACCENT, WHITE, BLEED_MARK_RATIO);

    for (suffix, size) in IOS_FILES {
        // `Spec::square` 自带 `opaque = true` → 走 RGB PNG，并在渲染后断言整幅不透明。
        let bytes = render(&spec, size)?;
        session.write("iOS", &ios.join(format!("AppIcon-{suffix}.png")), &bytes)?;
    }

    let mut contents = String::from("{\n  \"images\" : [\n");
    for (index, (idiom, size, scale, suffix)) in IOS_ENTRIES.iter().enumerate() {
        contents.push_str(&format!(
            "    {{\n      \"filename\" : \"AppIcon-{suffix}.png\",\n      \"idiom\" : \"{idiom}\",\n      \"scale\" : \"{scale}\",\n      \"size\" : \"{size}\"\n    }}{}\n",
            if index + 1 == IOS_ENTRIES.len() { "" } else { "," }
        ));
    }
    contents.push_str("  ],\n  \"info\" : {\n    \"author\" : \"icon-gen\",\n    \"version\" : 1\n  }\n}\n");
    session.write("iOS", &ios.join("Contents.json"), contents.as_bytes())?;
    Ok(())
}

/// 7) Web：favicon、PWA 图标、maskable、可随主题变色的矢量。
fn emit_web(out: &Path, session: &mut Session) -> Result<()> {
    let web = out.join("web");
    let solid = Spec::solid();

    for size in [16u32, 32, 48] {
        session.write(
            "Web",
            &web.join(format!("favicon-{size}.png")),
            &render(&solid, size)?,
        )?;
    }

    let favicon = build_ico(&solid, &FAVICON_SIZES)?;
    session.write("Web", &web.join("favicon.ico"), &favicon)?;

    // iOS Safari 的书签图标同样不能带透明通道，因此用满幅方形。
    let touch = Spec::square(ACCENT, WHITE, BLEED_MARK_RATIO);
    session.write(
        "Web",
        &web.join("apple-touch-icon-180.png"),
        &render(&touch, 180)?,
    )?;

    for size in [192u32, 512] {
        session.write(
            "Web",
            &web.join(format!("icon-{size}.png")),
            &render(&solid, size)?,
        )?;
    }

    // maskable：整幅铺底色，标记压进中央安全圆内。
    let maskable = Spec::square(ACCENT, WHITE, SAFE_CIRCLE_MARK_RATIO);
    session.write(
        "Web",
        &web.join("icon-maskable-512.png"),
        &render(&maskable, 512)?,
    )?;

    session.write("Web", &web.join("icon.svg"), theme_aware_svg().as_bytes())?;
    session.write(
        "Web",
        &web.join("site.webmanifest"),
        WEBMANIFEST.as_bytes(),
    )?;
    Ok(())
}

/// 8) 变体：深色 / 浅色 / 透明背景。
fn emit_variants(out: &Path, session: &mut Session) -> Result<()> {
    let variants = out.join("variants");
    let sets: [(&str, Spec, &str); 4] = [
        ("dark", Spec::deep(), "icon-c-folder-deep"),
        ("light", Spec::tinted(), "icon-c-folder-tinted"),
        ("transparent", Spec::glyph(ACCENT), "icon-c-folder-glyph"),
        (
            "transparent",
            Spec::glyph(WHITE),
            "icon-c-folder-glyph-white",
        ),
    ];

    for (group, spec, name) in sets {
        for size in [256u32, 512, 1024] {
            session.write(
                "变体",
                &variants.join(group).join(format!("{name}-{size}.png")),
                &render(&spec, size)?,
            )?;
        }
    }
    Ok(())
}

/// 渲染一个规格到目标尺寸，返回 PNG 字节。
///
/// 输出是否带 alpha 通道由规格自己决定（[`Spec::opaque`]）：
/// 满幅形态（iOS AppIcon、Apple Touch Icon、maskable）走无 alpha 的 RGB PNG，
/// 其余走 RGBA，好让图标能叠在任何底色上。
fn render(spec: &Spec, size: u32) -> Result<Vec<u8>> {
    let spec = spec.for_size(size);
    let vector = Vector::parse(&spec.svg("叠页"))?;
    let pixmap = vector.render(size)?;

    if spec.opaque {
        if !raster::is_fully_opaque(&pixmap) {
            bail!("{size}px 要求整幅不透明，但渲染结果存在透明像素");
        }
        raster::png_rgb(&pixmap)
    } else {
        raster::png_rgba(&pixmap)
    }
}

/// 组装 `.ico`：小尺寸用 BMP（兼容老程序），大尺寸用 PNG（体积小）。
fn build_ico(spec: &Spec, sizes: &[u32]) -> Result<Vec<u8>> {
    let mut images = Vec::with_capacity(sizes.len());
    for size in sizes {
        let adjusted = spec.for_size(*size);
        let vector = Vector::parse(&adjusted.svg("叠页"))?;
        let pixmap = vector.render(*size)?;
        let payload = if *size <= 64 {
            raster::bmp_payload(&pixmap)
        } else {
            raster::png_rgba(&pixmap)?
        };
        images.push(IcoImage {
            size: *size,
            payload,
        });
    }
    containers::write_ico(&images)
}

/// Web 用的矢量：内嵌 `prefers-color-scheme`，深色模式下自动换成深色底板。
///
/// 这是唯一一份「同一文件适配深浅色」的产物，因此单独生成而不复用 [`Spec::svg`]：
/// 后者刻意不引入 CSS（`resvg` 不解析媒体查询，两套逻辑混在一起会很难查）。
fn theme_aware_svg() -> String {
    let deep = design::deep_plate();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 1024 1024\" width=\"1024\" height=\"1024\" role=\"img\" aria-label=\"叠页\">\n\
         \x20 <title>叠页 · 层叠文件夹</title>\n\
         \x20 <style>\n\
         \x20   :root {{ --plate: {accent}; --mark: {mark}; }}\n\
         \x20   @media (prefers-color-scheme: dark) {{ :root {{ --plate: {deep}; }} }}\n\
         \x20   .plate {{ fill: var(--plate); }}\n\
         \x20   .mark {{ fill: var(--mark); stroke: var(--plate); }}\n\
         \x20 </style>\n\
         \x20 <path class=\"plate\" d=\"{plate}\"/>\n\
         \x20 <g class=\"mark\" transform=\"translate(512 512) scale(1.08) translate(-512 -512)\" stroke-width=\"20\">\n\
         \x20   <path d=\"{back}\"/>\n\
         \x20   <path d=\"{front}\"/>\n\
         \x20 </g>\n\
         </svg>\n",
        accent = ACCENT.hex(),
        mark = WHITE.hex(),
        deep = deep.hex(),
        plate = design::PLATE_PATH,
        back = design::FOLDER_BACK,
        front = design::FOLDER_FRONT,
    )
}

/// PWA 清单里只列图标本体相关的字段：其余字段（名称、主题色）属于站点自身配置。
const WEBMANIFEST: &str = r#"{
  "icons": [
    { "src": "icon-192.png", "sizes": "192x192", "type": "image/png" },
    { "src": "icon-512.png", "sizes": "512x512", "type": "image/png" },
    { "src": "icon-maskable-512.png", "sizes": "512x512", "type": "image/png", "purpose": "maskable" },
    { "src": "icon.svg", "sizes": "any", "type": "image/svg+xml" }
  ]
}
"#;

/// 仓库根目录：本工具的清单在 `tools/icon-gen/`，上溯两级。
fn repo_root() -> Result<PathBuf> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .context("无法从工具所在目录上溯到仓库根")
}

/// 记录写出的文件，便于最后汇总。
struct Session {
    root: PathBuf,
    groups: BTreeMap<&'static str, (usize, u64)>,
}

impl Session {
    fn new(root: PathBuf) -> Self {
        Self {
            root,
            groups: BTreeMap::new(),
        }
    }

    /// 写一个文件并计入所属分组。
    fn write(&mut self, group: &'static str, path: &Path, bytes: &[u8]) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("创建目录失败：{}", parent.display()))?;
        }
        fs::write(path, bytes).with_context(|| format!("写入失败：{}", path.display()))?;

        let entry = self.groups.entry(group).or_insert((0, 0));
        entry.0 += 1;
        entry.1 += bytes.len() as u64;
        Ok(())
    }

    /// 打印汇总与校验结果。
    fn report(&self, checks: &[checks::Check]) {
        let total_files: usize = self.groups.values().map(|(count, _)| count).sum();
        let total_bytes: u64 = self.groups.values().map(|(_, bytes)| bytes).sum();

        println!("\n叠页 · 层叠文件夹（设计稿 C 方案）");
        println!("输出目录：{}", self.root.join(OUT_DIR).display());
        println!(
            "共 {} 个文件 · {}",
            total_files,
            human(total_bytes)
        );
        for (group, (count, bytes)) in &self.groups {
            println!("  {group:<12} {count:>3} 个 · {}", human(*bytes));
        }

        println!("\n校验（{} 项）", checks.len());
        for check in checks {
            println!(
                "  [{}] {} — {}",
                if check.passed { "PASS" } else { "FAIL" },
                check.label,
                check.detail
            );
        }
        if checks.iter().all(|check| check.passed) {
            println!("\n全部通过。");
        }
    }
}

/// 人类可读的体积。
fn human(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.2} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}
