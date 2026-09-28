//! 外观档位：界面密度与缩略图大小。
//!
//! 之所以放在 core 而不是视图层：这两个档位需要写进配置文件（`fm-services::config`
//! 只依赖 core），而具体像素值又只对视图层有意义。折中做法是把「档位 → 像素」的
//! 换算放在这里——它是纯函数，不依赖任何 UI 框架，正好符合 core 的约束。
//!
//! 像素取值直接来自设计稿 `design/file-manager.html`：
//! `html[data-density=compact]{--row-h:27px}` / `[data-zoom="0"]{--cell:122px}`。

use serde::{Deserialize, Serialize};

/// 界面密度。影响列表行高，也间接影响骨架行与缩略图间距。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Density {
    /// 紧凑（行高 27px）。
    Compact,
    /// 标准（行高 32px），设计稿默认值。
    #[default]
    Normal,
    /// 宽松（行高 38px）。
    Cozy,
}

impl Density {
    /// 列表行高。
    pub fn row_height(self) -> f32 {
        match self {
            Self::Compact => 27.0,
            Self::Normal => 32.0,
            Self::Cozy => 38.0,
        }
    }

    /// 供设置界面与状态栏使用的文案。
    pub fn label(self) -> &'static str {
        match self {
            Self::Compact => "紧凑",
            Self::Normal => "标准",
            Self::Cozy => "宽松",
        }
    }
}

/// 缩略图档位。对应设计稿里状态栏的缩放滑块（三档）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum IconZoom {
    /// 小（单元格 122px）。
    Small,
    /// 中（单元格 152px），设计稿默认值。
    #[default]
    Medium,
    /// 大（单元格 192px）。
    Large,
}

impl IconZoom {
    /// 图标视图的单元格边长。
    pub fn cell_size(self) -> f32 {
        match self {
            Self::Small => 122.0,
            Self::Medium => 152.0,
            Self::Large => 192.0,
        }
    }

    /// 状态栏三档控件的下标，供「第几档」比较使用。
    pub fn index(self) -> usize {
        match self {
            Self::Small => 0,
            Self::Medium => 1,
            Self::Large => 2,
        }
    }

    /// 由下标还原档位（越界时退回中档）。
    pub fn from_index(index: usize) -> Self {
        match index {
            0 => Self::Small,
            2 => Self::Large,
            _ => Self::Medium,
        }
    }

    /// 三档的全部取值，供状态栏按序渲染控件。
    pub const ALL: [IconZoom; 3] = [IconZoom::Small, IconZoom::Medium, IconZoom::Large];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_height_matches_the_design_tokens() {
        assert_eq!(Density::Compact.row_height(), 27.0);
        assert_eq!(Density::Normal.row_height(), 32.0);
        assert_eq!(Density::Cozy.row_height(), 38.0);
        // 默认档必须与设计稿的 `data-density="normal"` 一致。
        assert_eq!(Density::default(), Density::Normal);
    }

    #[test]
    fn cell_size_matches_the_design_tokens() {
        assert_eq!(IconZoom::Small.cell_size(), 122.0);
        assert_eq!(IconZoom::Medium.cell_size(), 152.0);
        assert_eq!(IconZoom::Large.cell_size(), 192.0);
        assert_eq!(IconZoom::default(), IconZoom::Medium);
    }

    #[test]
    fn index_round_trips_for_every_zoom() {
        for zoom in IconZoom::ALL {
            assert_eq!(IconZoom::from_index(zoom.index()), zoom);
        }
        // 越界下标退回中档，而不是 panic。
        assert_eq!(IconZoom::from_index(9), IconZoom::Medium);
    }
}
