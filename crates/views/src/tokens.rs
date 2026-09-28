//! 设计令牌（design tokens）：配色、字号、间距与尺寸的**唯一**来源。
//!
//! 取值逐条抄自 `design/file-manager.html` 的 `:root` 与 `html[data-theme=dark]`，
//! 以及 `file-manager-settings.html` 的同名列。设计稿用 OKLCH 表达颜色，
//! 而 GPUI 的 `Hsla` 需要 HSL，所以这里做一次转换（见 [`oklch_to_hsla`]）。
//!
//! 为什么不直接用 `gpui_component::Theme`：
//! 组件库自带主题的色值与设计稿并不一致（例如它的 `accent` 偏紫），逐字段覆盖它
//! 既覆盖不全、也无法表达 `accent-soft` 这类「带透明度的派生色」。
//! 因此本项目自持一份令牌，只在需要时把关键字段同步给组件库（见 `fm_shell` 的
//! `apply_component_theme`），保证内建控件的观感不跑偏。
//!
//! 派生色的算法与设计稿一致，都是「原色 + 透明度」：
//! `color-mix(in oklch, X 16%, transparent)` 等价于 `X` 的 alpha 取 16%。

use fm_core::{Density, IconZoom};
use gpui::{Hsla, SharedString, hsla};

/// 布局常量：全部取自设计稿，单位 px。
///
/// 集中成模块而不是散落在各渲染函数里，是为了让「设计稿改了 8px」这样的调整
/// 只需要改一行，而不是全局搜索魔法数字。
pub mod metrics {
    /// 自定义标题栏高度。
    /// 品牌栏（标题栏）高度。
    ///
    /// 这一栏目前**没有在使用**：它原来放品牌标志、就绪指示与窗口按钮，
    /// 按反馈整条删除（窗口拖拽区与窗口按钮都移到了标签条那一栏）。
    /// 常量保留是因为它属于设计稿的取值，重新加回这一栏时要用它。
    pub const TITLEBAR_HEIGHT: f32 = 38.0;
    /// 标签条高度。
    pub const TABSTRIP_HEIGHT: f32 = 38.0;
    /// 单个标签高度。
    pub const TAB_HEIGHT: f32 = 32.0;
    /// 标签最小 / 最大宽度（过长时截断，过短时不留空隙）。
    pub const TAB_MIN_WIDTH: f32 = 112.0;
    pub const TAB_MAX_WIDTH: f32 = 196.0;
    /// 标签条顶部内边距。
    pub const TABSTRIP_PADDING_TOP: f32 = 6.0;
    /// 命令条内边距。
    pub const TOOLBAR_PADDING_X: f32 = 10.0;
    pub const TOOLBAR_PADDING_Y: f32 = 7.0;
    /// 图标按钮边长。
    pub const CONTROL_SIZE: f32 = 30.0;
    /// 命令条内分段控件里的按钮边长。
    pub const SEGMENT_CONTROL_SIZE: f32 = 26.0;
    /// 地址栏高度与最小宽度。
    pub const ADDRESS_HEIGHT: f32 = 32.0;
    pub const ADDRESS_MIN_WIDTH: f32 = 240.0;
    /// 面包屑单段高度与圆角。
    pub const CRUMB_HEIGHT: f32 = 24.0;
    pub const CRUMB_RADIUS: f32 = 5.0;
    /// 列表上方那条横栏的高度：**选中条与预览面板表头共用这一个值**。
    ///
    /// 定死高度而不是靠内边距撑出来：两栏并排，差一两个像素就会被看出来，
    /// 而文字行高由字体度量决定——两边各撑各的，迟早会因为换个字体或改个字号而错开。
    pub const BATCH_BAR_HEIGHT: f32 = 40.0;
    /// 内联重命名输入框的圆角（设计稿 `.rename` 用的是 5px 这一档）。
    pub const RENAME_RADIUS: f32 = 5.0;
    /// 搜索框宽度区间。
    pub const SEARCH_WIDTH: f32 = 230.0;
    pub const SEARCH_MIN_WIDTH: f32 = 132.0;
    /// 侧边栏展开 / 折叠宽度。
    pub const SIDEBAR_WIDTH: f32 = 238.0;
    pub const SIDEBAR_COLLAPSED_WIDTH: f32 = 48.0;
    /// 侧边栏导航项高度与子项缩进。
    pub const NAV_ITEM_HEIGHT: f32 = 30.0;
    pub const NAV_SUB_INDENT: f32 = 30.0;
    /// 详情视图表头高度。
    pub const LIST_HEAD_HEIGHT: f32 = 28.0;
    /// 详情视图各列宽度（四档响应式分别取用）。
    pub const CHECKBOX_COLUMN: f32 = 30.0;
    pub const DATE_COLUMN: f32 = 138.0;
    pub const DATE_COLUMN_NARROW: f32 = 126.0;
    pub const TYPE_COLUMN: f32 = 104.0;
    pub const SIZE_COLUMN: f32 = 86.0;
    pub const SIZE_COLUMN_NARROW: f32 = 84.0;
    pub const SIZE_COLUMN_TINY: f32 = 72.0;
    /// 行内左右内边距。
    pub const ROW_PADDING_X: f32 = 12.0;
    /// 右侧预览面板宽度（窄屏浮层时略窄）。
    pub const PREVIEW_WIDTH: f32 = 306.0;
    pub const PREVIEW_OVERLAY_WIDTH: f32 = 290.0;
    /// 底部状态栏高度。
    pub const STATUS_BAR_HEIGHT: f32 = 26.0;
    /// 侧边栏驱动器容量条高度。
    pub const DISK_METER_HEIGHT: f32 = 6.0;
    /// 圆角。
    pub const RADIUS: f32 = 6.0;
    pub const RADIUS_LG: f32 = 10.0;
    /// 字号（设计稿 --fs-*）。
    pub const FONT_XS: f32 = 11.0;
    pub const FONT_SM: f32 = 12.0;
    pub const FONT_UI: f32 = 13.0;
    pub const FONT_LG: f32 = 15.0;
    pub const FONT_XL: f32 = 17.0;
    /// 图标视图单元格内的缩略图与文字间距。
    pub const CELL_GAP: f32 = 8.0;
    pub const CELL_PADDING: f32 = 8.0;
    /// 网格视图的单元格间距。
    pub const GRID_GAP: f32 = 10.0;
    pub const GRID_PADDING: f32 = 12.0;
    /// 窗口最小尺寸：比最窄断点再小一些，保证任何尺寸下布局都不散架。
    pub const WINDOW_MIN_WIDTH: f32 = 520.0;
    pub const WINDOW_MIN_HEIGHT: f32 = 400.0;
}

/// 响应式断点。数值与设计稿的 `@media` 一一对应。
pub mod breakpoints {
    /// ≤1240：隐藏「类型」列，日期列收窄。
    pub const HIDE_TYPE: f32 = 1240.0;
    /// ≤1100：预览面板改为浮层。
    pub const PREVIEW_OVERLAY: f32 = 1100.0;
    /// ≤980：隐藏「修改日期」列。
    pub const HIDE_DATE: f32 = 980.0;
    /// ≤920：侧边栏改为浮层。
    pub const SIDEBAR_OVERLAY: f32 = 920.0;
    /// ≤720：隐藏同步状态与状态栏的视图/缩放控件；隐藏中间层级的面包屑。
    pub const COMPACT: f32 = 720.0;
    /// ≤560：品牌文字隐藏，单元格缩小。
    pub const NARROW: f32 = 560.0;
}

/// 一套完整的运行时设计令牌。
#[derive(Clone, Debug)]
pub struct Tokens {
    /// 页面底色（设计稿 --bg）。
    pub bg: Hsla,
    /// 内容区表面色（--surface）。
    pub surface: Hsla,
    /// 侧边栏 / 标签条 / 状态栏底色（--panel）。
    pub panel: Hsla,
    /// 主前景色（--fg）。
    pub fg: Hsla,
    /// 次级前景色（--muted）。
    pub muted_fg: Hsla,
    /// 分隔线（--border）。
    pub border: Hsla,
    /// 强调色（--accent）。
    pub accent: Hsla,
    /// 警告色（--warn），容量条高占用时使用。
    pub warn: Hsla,
    /// 危险色（--danger）。
    pub danger: Hsla,
    /// 强调色 16% 透明底（--accent-soft）：选中行背景。
    pub accent_soft: Hsla,
    /// 强调色 45% 透明描边（--accent-line）：焦点环与选中单元格边框。
    pub accent_line: Hsla,
    /// 前景色 6% 透明底（--hover）：悬停背景。
    pub hover: Hsla,
    /// 危险色 16% 透明底（--danger-soft）。
    pub danger_soft: Hsla,
    /// 是否深色模式。少数地方需要据此微调（例如缩略图底板）。
    pub dark: bool,
    /// 列表行高（随密度变化）。
    pub row_height: f32,
    /// 图标视图单元格边长（随缩放档位变化）。
    pub cell: f32,
    /// 界面字体族。
    pub font_family: SharedString,
    /// 等宽字体族：所有数字列与短标签都用它，让列对齐。
    pub mono_font: SharedString,
}

impl Tokens {
    /// 按深浅模式、密度与缩放档位构建令牌。
    ///
    /// 字体族由调用方从组件库主题里取——令牌层不认识 `gpui_component::Theme`，
    /// 因此这里只接收结果，不产生依赖。
    pub fn new(
        dark: bool,
        density: Density,
        zoom: IconZoom,
        font_family: impl Into<SharedString>,
        mono_font: impl Into<SharedString>,
    ) -> Self {
        let mut tokens = if dark { Self::dark() } else { Self::light() };
        tokens.dark = dark;
        tokens.row_height = density.row_height();
        tokens.cell = zoom.cell_size();
        tokens.font_family = font_family.into();
        tokens.mono_font = mono_font.into();
        tokens
    }

    /// 浅色令牌（设计稿默认）。
    pub fn light() -> Self {
        let fg = oklch_to_hsla(22.0, 0.02, 240.0);
        let accent = oklch_to_hsla(55.0, 0.2, 258.0);
        let danger = oklch_to_hsla(56.0, 0.19, 27.0);
        Self {
            bg: oklch_to_hsla(98.0, 0.005, 250.0),
            surface: oklch_to_hsla(100.0, 0.0, 0.0),
            panel: oklch_to_hsla(96.5, 0.006, 250.0),
            fg,
            muted_fg: oklch_to_hsla(50.0, 0.018, 240.0),
            border: oklch_to_hsla(90.0, 0.008, 240.0),
            accent,
            warn: oklch_to_hsla(72.0, 0.15, 78.0),
            danger,
            accent_soft: with_alpha(accent, 0.16),
            accent_line: with_alpha(accent, 0.45),
            hover: with_alpha(fg, 0.06),
            danger_soft: with_alpha(danger, 0.16),
            dark: false,
            row_height: Density::default().row_height(),
            cell: IconZoom::default().cell_size(),
            font_family: ".SystemUIFont".into(),
            mono_font: "Consolas".into(),
        }
    }

    /// 深色令牌。
    pub fn dark() -> Self {
        let fg = oklch_to_hsla(94.0, 0.006, 250.0);
        let accent = oklch_to_hsla(73.0, 0.16, 258.0);
        let danger = oklch_to_hsla(72.0, 0.17, 27.0);
        Self {
            bg: oklch_to_hsla(18.0, 0.012, 250.0),
            surface: oklch_to_hsla(25.0, 0.014, 250.0),
            panel: oklch_to_hsla(21.5, 0.013, 250.0),
            fg,
            muted_fg: oklch_to_hsla(68.0, 0.014, 250.0),
            border: oklch_to_hsla(32.0, 0.012, 250.0),
            accent,
            warn: oklch_to_hsla(80.0, 0.14, 78.0),
            danger,
            accent_soft: with_alpha(accent, 0.16),
            accent_line: with_alpha(accent, 0.45),
            hover: with_alpha(fg, 0.06),
            danger_soft: with_alpha(danger, 0.16),
            dark: true,
            row_height: Density::default().row_height(),
            cell: IconZoom::default().cell_size(),
            font_family: ".SystemUIFont".into(),
            mono_font: "Consolas".into(),
        }
    }

    /// 「已用容量」条的前景色：占用超过 85% 时转为警告色。
    ///
    /// 阈值取自设计稿的 `p > 85 ? 'u warn' : 'u'`。
    pub fn capacity_fill(&self, used_ratio: f32) -> Hsla {
        if used_ratio > 0.85 {
            self.warn
        } else {
            self.accent
        }
    }
}

/// 把颜色替换成指定透明度，等价于设计稿里的 `color-mix(..., X% , transparent)`。
pub fn with_alpha(color: Hsla, alpha: f32) -> Hsla {
    Hsla { a: alpha, ..color }
}

/// OKLCH → HSL。
///
/// `lightness` 用百分数（设计稿写 `55%`，这里传 `55.0`），`chroma` 传小数，
/// `hue` 传角度。超出 sRGB 色域的分量会被夹到 0..=1——设计稿的取值都在色域内，
/// 但用户改主题时可能越界，夹一下比画出一个溢出通道安全。
pub fn oklch_to_hsla(lightness: f32, chroma: f32, hue: f32) -> Hsla {
    let (red, green, blue) = oklch_to_srgb(lightness / 100.0, chroma, hue);
    srgb_to_hsla(red, green, blue)
}

/// OKLCH → sRGB（每个分量 0..=1）。
///
/// 转换链：OKLCH → OKLab → LMS' → LMS → 线性 sRGB → gamma 编码 sRGB。
/// 系数取自 Björn Ottosson 的 OKLab 定义，与 CSS `oklch()` 的实现一致。
fn oklch_to_srgb(lightness: f32, chroma: f32, hue: f32) -> (f32, f32, f32) {
    let hue = hue.to_radians();
    let a = chroma * hue.cos();
    let b = chroma * hue.sin();

    let l = lightness + 0.396_337_78 * a + 0.215_803_76 * b;
    let m = lightness - 0.105_561_346 * a - 0.063_854_17 * b;
    let s = lightness - 0.089_484_18 * a - 1.291_485_5 * b;

    let l = l * l * l;
    let m = m * m * m;
    let s = s * s * s;

    let red = 4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s;
    let green = -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s;
    let blue = -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s;

    (encode_gamma(red), encode_gamma(green), encode_gamma(blue))
}

/// 线性 → gamma 编码。负值（超出色域）直接夹到 0，避免 `powf` 产生 NaN。
fn encode_gamma(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    if value <= 0.003_130_8 {
        12.92 * value
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    }
}

/// sRGB → HSL。
///
/// **单位陷阱**：GPUI 的 `Hsla.h` 取值是 **0..1**，而不是 CSS / 教科书里的 0..360。
/// 因此最后一步必须除以 360；漏掉它会被 `hsla()` 全部夹到 1.0，
/// 得到一组「色相全是紫红」的颜色——这个错误不会报错，只会悄悄画错。
fn srgb_to_hsla(red: f32, green: f32, blue: f32) -> Hsla {
    let max = red.max(green).max(blue);
    let min = red.min(green).min(blue);
    let lightness = (max + min) / 2.0;
    let delta = max - min;

    if delta <= f32::EPSILON {
        // 无彩色：色相与饱和度都无意义，取 0 而不是留一个除零产生的 NaN。
        return hsla(0.0, 0.0, lightness, 1.0);
    }

    let saturation = delta / (1.0 - (2.0 * lightness - 1.0).abs());
    let hue = if max == red {
        60.0 * (((green - blue) / delta) % 6.0)
    } else if max == green {
        60.0 * ((blue - red) / delta + 2.0)
    } else {
        60.0 * ((red - green) / delta + 4.0)
    };

    // 见函数文档：GPUI 的色相是 0..1，必须先除以 360。
    hsla(hue.rem_euclid(360.0) / 360.0, saturation, lightness, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 把 Hsla 换算回 8 位 sRGB，便于与设计稿给出的十六进制值对照。
    fn to_rgb8(color: Hsla) -> (u8, u8, u8) {
        // `color.h` 是 0..1，标准公式用的是角度，这里换算回 0..360。
        let hue = color.h * 360.0;
        let hsl_to_rgb = |n: f32| {
            let k = (n + hue / 30.0) % 12.0;
            let a = color.s * color.l.min(1.0 - color.l);
            color.l - a * (-1.0_f32).max((k - 3.0).min((9.0 - k).min(1.0)))
        };
        let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
        (
            channel(hsl_to_rgb(0.0)),
            channel(hsl_to_rgb(8.0)),
            channel(hsl_to_rgb(4.0)),
        )
    }

    fn assert_close(actual: (u8, u8, u8), expected: (u8, u8, u8), tolerance: u8) {
        let deltas = [
            actual.0.abs_diff(expected.0),
            actual.1.abs_diff(expected.1),
            actual.2.abs_diff(expected.2),
        ];
        assert!(
            deltas.iter().all(|delta| *delta <= tolerance),
            "实际 {actual:?} 与设计稿 {expected:?} 偏差超过 {tolerance}"
        );
    }

    #[test]
    fn accent_matches_the_color_published_with_the_design() {
        // design/app-icons.html 明确写了 `oklch(55% 0.2 258)` 换算成 sRGB 是 #076be3。
        // 这条断言同时校验了 OKLCH 转换链与 HSL 往返两条路径。
        assert_close(to_rgb8(Tokens::light().accent), (0x07, 0x6b, 0xe3), 2);
    }

    #[test]
    fn opaque_tokens_have_full_alpha() {
        for token in [Tokens::light(), Tokens::dark()] {
            for color in [token.bg, token.surface, token.fg, token.border, token.accent] {
                assert_eq!(color.a, 1.0);
            }
        }
    }

    #[test]
    fn derived_tokens_keep_the_base_hue_and_the_designed_alpha() {
        let light = Tokens::light();
        // 派生色只改透明度，色相必须与基色一致，否则选中态会偏色。
        assert_eq!(light.accent_soft.h, light.accent.h);
        assert_eq!(light.accent_line.h, light.accent.h);
        assert_eq!(light.hover.h, light.fg.h);
        assert!((light.accent_soft.a - 0.16).abs() < f32::EPSILON);
        assert!((light.accent_line.a - 0.45).abs() < f32::EPSILON);
        assert!((light.hover.a - 0.06).abs() < f32::EPSILON);
    }

    #[test]
    fn dark_and_light_are_actually_different() {
        let light = Tokens::light();
        let dark = Tokens::dark();
        assert!(!light.dark && dark.dark);
        // 深浅两套必须真的分得开，否则说明有一侧的取值被写错了。
        assert!(light.surface.l > dark.surface.l);
        assert!(light.fg.l < dark.fg.l);
    }

    #[test]
    fn density_and_zoom_are_applied_from_the_enum() {
        let tokens = Tokens::new(
            false,
            Density::Cozy,
            IconZoom::Small,
            ".SystemUIFont",
            "Consolas",
        );
        assert_eq!(tokens.row_height, 38.0);
        assert_eq!(tokens.cell, 122.0);
    }
}
