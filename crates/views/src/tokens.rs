//! Fluent 设计令牌：配色、字体、尺寸与响应式断点的唯一来源。
//!
//! 取值逐条对应 `design/pagefold-file-manager.html`。设计稿使用固定 sRGB 色板，
//! 这里只在构建令牌时做一次 HEX → HSL 转换；列表行不会逐行解析颜色。

use fm_core::{Density, IconZoom};
use gpui::{Hsla, SharedString, hsla};

/// 布局常量，单位 px。
pub mod metrics {
    pub const TITLEBAR_HEIGHT: f32 = 40.0;
    pub const TABSTRIP_HEIGHT: f32 = 40.0;
    pub const TAB_HEIGHT: f32 = 36.0;
    pub const TAB_MIN_WIDTH: f32 = 144.0;
    pub const TAB_MIN_WIDTH_COMPACT: f32 = 112.0;
    pub const TAB_MAX_WIDTH: f32 = 220.0;
    pub const TABSTRIP_PADDING_TOP: f32 = 4.0;

    pub const TOOLBAR_HEIGHT: f32 = 48.0;
    pub const TOOLBAR_PADDING_X: f32 = 8.0;
    pub const TOOLBAR_PADDING_Y: f32 = 4.0;
    pub const CONTROL_SIZE: f32 = 32.0;
    pub const SEGMENT_CONTROL_SIZE: f32 = 26.0;
    pub const ADDRESS_HEIGHT: f32 = 32.0;
    pub const ADDRESS_MIN_WIDTH: f32 = 160.0;
    pub const ADDRESS_MIN_WIDTH_COMPACT: f32 = 180.0;
    pub const CRUMB_HEIGHT: f32 = 24.0;
    pub const CRUMB_RADIUS: f32 = 4.0;
    pub const BATCH_BAR_HEIGHT: f32 = 40.0;
    pub const RENAME_RADIUS: f32 = 4.0;
    pub const SEARCH_WIDTH: f32 = 230.0;
    pub const SEARCH_WIDTH_MEDIUM: f32 = 190.0;
    pub const SEARCH_WIDTH_NARROW: f32 = 160.0;
    pub const SEARCH_MIN_WIDTH: f32 = 140.0;

    pub const SIDEBAR_WIDTH: f32 = 236.0;
    pub const SIDEBAR_MEDIUM_WIDTH: f32 = 208.0;
    pub const SIDEBAR_COLLAPSED_WIDTH: f32 = 52.0;
    pub const NAV_ITEM_HEIGHT: f32 = 32.0;
    pub const NAV_SUB_INDENT: f32 = 32.0;

    pub const LIST_HEAD_HEIGHT: f32 = 32.0;
    pub const CHECKBOX_COLUMN: f32 = 30.0;
    pub const DATE_COLUMN: f32 = 156.0;
    pub const DATE_COLUMN_MEDIUM: f32 = 140.0;
    pub const DATE_COLUMN_NARROW: f32 = 132.0;
    pub const TYPE_COLUMN: f32 = 140.0;
    pub const TYPE_COLUMN_NARROW: f32 = 104.0;
    pub const SIZE_COLUMN: f32 = 92.0;
    pub const ROW_PADDING_X: f32 = 12.0;

    pub const PREVIEW_WIDTH: f32 = 304.0;
    pub const PREVIEW_MEDIUM_WIDTH: f32 = 272.0;
    pub const PREVIEW_NARROW_WIDTH: f32 = 260.0;
    pub const PREVIEW_OVERLAY_WIDTH: f32 = 304.0;
    pub const STATUS_BAR_HEIGHT: f32 = 28.0;
    pub const DISK_METER_HEIGHT: f32 = 5.0;

    pub const RADIUS_SM: f32 = 2.0;
    pub const RADIUS: f32 = 4.0;
    pub const RADIUS_MD: f32 = 6.0;
    pub const RADIUS_LG: f32 = 8.0;
    pub const RADIUS_XL: f32 = 12.0;

    pub const FONT_XS: f32 = 10.0;
    pub const FONT_SM: f32 = 12.0;
    pub const FONT_UI: f32 = 14.0;
    pub const FONT_LG: f32 = 16.0;
    pub const FONT_XL: f32 = 20.0;
    pub const FONT_XXL: f32 = 24.0;

    pub const CELL_GAP: f32 = 8.0;
    pub const CELL_PADDING: f32 = 8.0;
    pub const GRID_GAP: f32 = 8.0;
    pub const GRID_PADDING: f32 = 12.0;
    pub const WINDOW_MIN_WIDTH: f32 = 520.0;
    pub const WINDOW_MIN_HEIGHT: f32 = 480.0;
}

/// 新设计稿的三个桌面断点。
pub mod breakpoints {
    /// ≤1180：收窄栏宽、隐藏大小列、命令条允许两行。
    pub const MEDIUM: f32 = 1180.0;
    /// ≤920：侧栏自动折叠为图标态，隐藏类型列。
    pub const SIDEBAR_COMPACT: f32 = 920.0;
    /// ≤720：隐藏侧栏、预览变浮层、只保留名称列。
    pub const COMPACT: f32 = 720.0;
}

/// 一套完整的运行时 Fluent 设计令牌。
#[derive(Clone, Debug)]
pub struct Tokens {
    /// NeutralBackground3：标签条与应用底层。
    pub bg: Hsla,
    /// NeutralBackground1：主内容、卡片、输入框与浮层。
    pub surface: Hsla,
    /// NeutralBackground2：侧栏、导航条、详情栏与状态栏。
    pub panel: Hsla,
    pub fg: Hsla,
    pub secondary_fg: Hsla,
    pub muted_fg: Hsla,
    pub disabled_fg: Hsla,
    /// NeutralStroke2：常规分隔线。
    pub border: Hsla,
    /// NeutralStroke1：输入框与浮层描边。
    pub border_strong: Hsla,
    /// NeutralStroke3：列表行细分隔线。
    pub border_subtle: Hsla,
    /// CompoundBrandForeground / Stroke：品牌前景与焦点描边。
    pub accent: Hsla,
    pub accent_hover: Hsla,
    /// BrandBackground2：选中背景。
    pub accent_soft: Hsla,
    pub accent_soft_hover: Hsla,
    /// BrandStroke2：选中描边。
    pub accent_line: Hsla,
    /// BrandBackground：主要按钮底色。
    pub brand: Hsla,
    pub brand_hover: Hsla,
    pub brand_pressed: Hsla,
    pub on_accent: Hsla,
    pub hover: Hsla,
    pub pressed: Hsla,
    pub selected: Hsla,
    pub focus: Hsla,
    pub overlay: Hsla,
    pub warn: Hsla,
    pub danger: Hsla,
    pub danger_hover: Hsla,
    pub danger_soft: Hsla,
    pub success: Hsla,
    pub meter: Hsla,
    pub dark: bool,
    pub row_height: f32,
    pub cell: f32,
    pub font_family: SharedString,
    /// 数字使用 Bahnschrift；等宽文本继续使用 Consolas。
    pub numeric_font: SharedString,
    pub mono_font: SharedString,
}

impl Tokens {
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

    pub fn light() -> Self {
        Self {
            bg: hex(0xf5f5f5),
            surface: hex(0xffffff),
            panel: hex(0xfafafa),
            fg: hex(0x242424),
            secondary_fg: hex(0x424242),
            muted_fg: hex(0x616161),
            disabled_fg: hex(0xbdbdbd),
            border: hex(0xe0e0e0),
            border_strong: hex(0xd1d1d1),
            border_subtle: hex(0xf0f0f0),
            accent: hex(0x0f6cbd),
            accent_hover: hex(0x115ea3),
            accent_soft: hex(0xebf3fc),
            accent_soft_hover: hex(0xcfe4fa),
            accent_line: hex(0xb4d6fa),
            brand: hex(0x0f6cbd),
            brand_hover: hex(0x115ea3),
            brand_pressed: hex(0x0c3b5e),
            on_accent: hex(0xffffff),
            hover: hex(0xf5f5f5),
            pressed: hex(0xe0e0e0),
            selected: hex(0xebebeb),
            focus: hex(0x000000),
            overlay: hsla(0.0, 0.0, 0.0, 0.4),
            warn: hex(0x817400),
            danger: hex(0xd13438),
            danger_hover: hex(0xbc2f32),
            danger_soft: with_alpha(hex(0xd13438), 0.16),
            success: hex(0x107c10),
            meter: hex(0x707070),
            dark: false,
            row_height: Density::default().row_height(),
            cell: IconZoom::default().cell_size(),
            font_family: "Segoe UI".into(),
            numeric_font: "Bahnschrift".into(),
            mono_font: "Consolas".into(),
        }
    }

    pub fn dark() -> Self {
        Self {
            bg: hex(0x141414),
            surface: hex(0x292929),
            panel: hex(0x1f1f1f),
            fg: hex(0xffffff),
            secondary_fg: hex(0xd6d6d6),
            muted_fg: hex(0xadadad),
            disabled_fg: hex(0x5c5c5c),
            border: hex(0x525252),
            border_strong: hex(0x666666),
            border_subtle: hex(0x3d3d3d),
            accent: hex(0x479ef5),
            accent_hover: hex(0x62abf5),
            accent_soft: hex(0x082338),
            accent_soft_hover: hex(0x0c3b5e),
            accent_line: hex(0x0e4775),
            brand: hex(0x115ea3),
            brand_hover: hex(0x0f6cbd),
            brand_pressed: hex(0x0c3b5e),
            on_accent: hex(0xffffff),
            hover: hex(0x383838),
            pressed: hex(0x2e2e2e),
            selected: hex(0x333333),
            focus: hex(0xffffff),
            overlay: hsla(0.0, 0.0, 0.0, 0.5),
            warn: hex(0x817400),
            danger: hex(0xd13438),
            danger_hover: hex(0xbc2f32),
            danger_soft: with_alpha(hex(0xd13438), 0.16),
            success: hex(0x107c10),
            meter: hex(0x999999),
            dark: true,
            row_height: Density::default().row_height(),
            cell: IconZoom::default().cell_size(),
            font_family: "Segoe UI".into(),
            numeric_font: "Bahnschrift".into(),
            mono_font: "Consolas".into(),
        }
    }

    pub fn capacity_fill(&self, used_ratio: f32) -> Hsla {
        if used_ratio > 0.85 {
            self.warn
        } else {
            self.meter
        }
    }
}

pub fn with_alpha(color: Hsla, alpha: f32) -> Hsla {
    Hsla { a: alpha, ..color }
}

/// 仅在令牌构建时调用，避免把颜色转换带入逐行渲染。
fn hex(value: u32) -> Hsla {
    let red = ((value >> 16) & 0xff) as f32 / 255.0;
    let green = ((value >> 8) & 0xff) as f32 / 255.0;
    let blue = (value & 0xff) as f32 / 255.0;
    srgb_to_hsla(red, green, blue)
}

/// GPUI 的 `Hsla.h` 使用 0..1，而不是 CSS 的 0..360。
fn srgb_to_hsla(red: f32, green: f32, blue: f32) -> Hsla {
    let max = red.max(green).max(blue);
    let min = red.min(green).min(blue);
    let lightness = (max + min) / 2.0;
    let delta = max - min;
    if delta <= f32::EPSILON {
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
    hsla(hue.rem_euclid(360.0) / 360.0, saturation, lightness, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn to_rgb8(color: Hsla) -> (u8, u8, u8) {
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

    fn luminance(color: Hsla) -> f32 {
        let rgb = color.to_rgb();
        let channel = |value: f32| {
            if value <= 0.039_28 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(rgb.r) + 0.7152 * channel(rgb.g) + 0.0722 * channel(rgb.b)
    }

    fn contrast(a: Hsla, b: Hsla) -> f32 {
        let (a, b) = (luminance(a), luminance(b));
        let (light, dark) = if a > b { (a, b) } else { (b, a) };
        (light + 0.05) / (dark + 0.05)
    }

    #[test]
    fn fluent_light_palette_matches_design() {
        let tokens = Tokens::light();
        assert_eq!(to_rgb8(tokens.accent), (0x0f, 0x6c, 0xbd));
        assert_eq!(to_rgb8(tokens.surface), (0xff, 0xff, 0xff));
        assert_eq!(to_rgb8(tokens.panel), (0xfa, 0xfa, 0xfa));
        assert_eq!(to_rgb8(tokens.accent_soft), (0xeb, 0xf3, 0xfc));
        assert_eq!(to_rgb8(tokens.border), (0xe0, 0xe0, 0xe0));
    }

    #[test]
    fn fluent_dark_palette_matches_design() {
        let tokens = Tokens::dark();
        assert_eq!(to_rgb8(tokens.accent), (0x47, 0x9e, 0xf5));
        assert_eq!(to_rgb8(tokens.surface), (0x29, 0x29, 0x29));
        assert_eq!(to_rgb8(tokens.panel), (0x1f, 0x1f, 0x1f));
        assert_eq!(to_rgb8(tokens.accent_soft), (0x08, 0x23, 0x38));
        assert_eq!(to_rgb8(tokens.border), (0x52, 0x52, 0x52));
    }

    #[test]
    fn semantic_states_are_not_legacy_alpha_derivatives() {
        for tokens in [Tokens::light(), Tokens::dark()] {
            for color in [
                tokens.surface,
                tokens.panel,
                tokens.hover,
                tokens.accent_soft,
            ] {
                assert_eq!(color.a, 1.0);
            }
            assert_ne!(tokens.hover, tokens.surface);
            assert_ne!(tokens.accent_soft_hover, tokens.accent_soft);
        }
    }

    #[test]
    fn text_and_hover_states_stay_readable() {
        for tokens in [Tokens::light(), Tokens::dark()] {
            assert!(contrast(tokens.fg, tokens.surface) >= 4.5);
            assert!(contrast(tokens.fg, tokens.hover) >= 4.5);
            assert!(contrast(tokens.muted_fg, tokens.surface) >= 4.5);
            assert!(contrast(tokens.on_accent, tokens.brand) >= 4.5);
        }
    }

    #[test]
    fn density_zoom_and_font_roles_are_applied() {
        let tokens = Tokens::new(
            false,
            Density::Cozy,
            IconZoom::Small,
            "Segoe UI",
            "Consolas",
        );
        assert_eq!(tokens.row_height, 38.0);
        assert_eq!(tokens.cell, 122.0);
        assert_eq!(tokens.font_family.as_ref(), "Segoe UI");
        assert_eq!(tokens.numeric_font.as_ref(), "Bahnschrift");
        assert_eq!(tokens.mono_font.as_ref(), "Consolas");
    }
}
