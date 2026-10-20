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

    /// 供设置界面使用的文案（状态栏只用下标，设置页要写清楚是哪一档）。
    pub fn label(self) -> &'static str {
        match self {
            Self::Small => "小",
            Self::Medium => "中",
            Self::Large => "大",
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

/// 详情表里**宽度可拖动**的三列。
///
/// 「名称」列不在这里：它吃掉剩余空间（`flex_grow`），没有固定宽度可拖。
/// 「勾选框」列是固定 30px 的命中区，拖它没有意义。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Column {
    Date,
    Kind,
    Size,
}

impl Column {
    /// 该列允许的宽度区间。
    ///
    /// 下限保证「修改日期」这类定长内容不被拖成一条缝；上限保证不会把名称列挤没。
    pub fn bounds(self) -> (f32, f32) {
        match self {
            Self::Date => (72.0, 240.0),
            Self::Kind => (72.0, 220.0),
            Self::Size => (64.0, 180.0),
        }
    }

    /// 名称列的最小宽度：三列固定宽之和不得把内容区挤到比它还窄。
    pub const NAME_MIN_WIDTH: f32 = 180.0;
}

/// 用户拖动表头之后留下的列宽偏好。
///
/// 存 `Option<f32>` 而不是 `Option<f32>` 的语义差异在这里很关键：
/// `None` = 「没拖过，用断点默认值」，而不是「宽度为零」。
/// 三列各自独立——某一列在窄屏被断点隐藏时，它的偏好仍然保留，
/// 宽屏回来时不会因为「隐藏期间被清空」而丢掉。
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ColumnWidths {
    pub date: Option<f32>,
    pub kind: Option<f32>,
    pub size: Option<f32>,
}

impl ColumnWidths {
    /// 取某一列的偏好宽度。
    pub fn get(self, column: Column) -> Option<f32> {
        match column {
            Column::Date => self.date,
            Column::Kind => self.kind,
            Column::Size => self.size,
        }
    }

    /// 写入某一列的宽度，并按 [`Column::bounds`] 夹取。
    ///
    /// 夹取放在这里而不是调用点：拖动过程、写配置、读配置三条路径都会进来，
    /// 漏掉任何一处，坏值就会在重启之后以「列宽离谱」的形式出现。
    pub fn set(&mut self, column: Column, width: f32) {
        let (min, max) = column.bounds();
        let width = width.clamp(min, max);
        match column {
            Column::Date => self.date = Some(width),
            Column::Kind => self.kind = Some(width),
            Column::Size => self.size = Some(width),
        }
    }

    /// 把三列分别夹回各自的区间。
    ///
    /// 配置是磁盘上的 JSON，可能被手工改过（也可能来自更宽容的旧版本），
    /// 因此**读取路径也必须夹一次**——只靠写入时夹，一个手改的 9999
    /// 就能让名称列消失到只剩负空间。
    pub fn clamped(self) -> Self {
        let mut widths = Self::default();
        for column in [Column::Date, Column::Kind, Column::Size] {
            if let Some(width) = self.get(column) {
                widths.set(column, width);
            }
        }
        widths
    }

    /// 按可用宽度收敛三列之和，返回实际应使用的宽度。
    ///
    /// 收敛顺序是「大小 → 类型 → 日期」：大小列的数值最短、压缩代价最小，
    /// 日期列的信息最不可替代，因此最后才动它。名称列的最小宽度优先于三者——
    /// 把名称挤没，整张表就没有用了。
    pub fn fitted(self, available: f32) -> Self {
        let mut widths = self.clamped();
        let budget = (available - Column::NAME_MIN_WIDTH).max(0.0);

        let total = |widths: &Self| {
            widths.date.unwrap_or(0.0) + widths.kind.unwrap_or(0.0) + widths.size.unwrap_or(0.0)
        };

        for column in [Column::Size, Column::Kind, Column::Date] {
            let excess = total(&widths) - budget;
            if excess <= 0.0 {
                break;
            }
            let Some(current) = widths.get(column) else {
                continue;
            };
            let (min, _) = column.bounds();
            let reduced = (current - excess).max(min);
            widths.set(column, reduced);
        }
        widths
    }
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

    #[test]
    fn column_widths_are_clamped_on_write() {
        let mut widths = ColumnWidths::default();
        // 下限：拖到负数或 0 也不能出现「宽度 0 的列」。
        widths.set(Column::Date, 1.0);
        assert_eq!(widths.date, Some(72.0));
        // 上限：不允许把名称列挤没。
        widths.set(Column::Kind, 9999.0);
        assert_eq!(widths.kind, Some(220.0));
        widths.set(Column::Size, 10.0);
        assert_eq!(widths.size, Some(64.0));
    }

    #[test]
    fn untouched_columns_stay_none() {
        let mut widths = ColumnWidths::default();
        widths.set(Column::Size, 90.0);
        assert_eq!(widths.date, None, "没拖过的列必须保持 None，才能回落到断点默认值");
        assert_eq!(widths.kind, None);
        assert_eq!(widths.size, Some(90.0));
    }

    #[test]
    fn fitted_shrinks_size_before_date() {
        let widths = ColumnWidths {
            date: Some(240.0),
            kind: Some(220.0),
            size: Some(180.0),
        };
        // 预算 = 480 - 名称列 180 = 300；三列之和 640，必须让出 340。
        let fitted = widths.fitted(480.0);
        assert_eq!(fitted.size, Some(64.0), "大小列应最先被压到下限");
        assert_eq!(fitted.kind, Some(72.0), "大小列让完还不够时才动类型列");
        assert_eq!(fitted.date, Some(164.0), "日期列最后才动，且只让出剩余部分");
        let total = fitted.date.unwrap() + fitted.kind.unwrap() + fitted.size.unwrap();
        assert!((total - 300.0).abs() < 0.01, "收敛后应刚好用满预算");
    }

    #[test]
    fn out_of_range_config_values_are_clamped_before_use() {
        // 手改配置塞进 9999：读取路径必须夹住，否则名称列会被挤到负空间。
        let widths = ColumnWidths {
            date: Some(9999.0),
            kind: Some(-5.0),
            size: Some(1.0),
        }
        .clamped();
        assert_eq!(widths.date, Some(240.0));
        assert_eq!(widths.kind, Some(72.0));
        assert_eq!(widths.size, Some(64.0));
    }

    #[test]
    fn fitted_leaves_widths_alone_when_they_fit() {
        let widths = ColumnWidths {
            date: Some(138.0),
            kind: Some(104.0),
            size: Some(86.0),
        };
        assert_eq!(widths.fitted(800.0), widths);
    }

    #[test]
    fn fitted_never_goes_below_the_minimums() {
        // 极端窄的内容区：收敛到下限就停，不会产生负宽度。
        let fitted = ColumnWidths {
            date: Some(240.0),
            kind: Some(220.0),
            size: Some(180.0),
        }
        .fitted(200.0);
        assert_eq!(fitted.date, Some(72.0));
        assert_eq!(fitted.kind, Some(72.0));
        assert_eq!(fitted.size, Some(64.0));
    }
}
