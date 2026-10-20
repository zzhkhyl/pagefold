//! 配置持久化。
//!
//! 存储位置：`%APPDATA%\pagefold\config.json`。
//!
//! 容错原则：配置是「锦上添花」而不是启动前提。
//! 文件缺失、内容损坏、字段新增/删除、写盘失败，都只记 WARN 日志并回退到默认值——
//! 一个坏掉的配置文件不应该让文件管理器打不开。
//!
//! 字段全部是**外观与偏好**：主题、密度、视图模式、面板开关。
//! 刻意不把「上次浏览到哪个目录」这类会话状态写进来：
//! 那属于标签页恢复，需要连同历史一起建模，否则恢复出来的位置没有任何上下文。

use std::fs;
use std::path::PathBuf;

use fm_core::{ColumnWidths, Density, IconZoom, ViewMode};
use serde::{Deserialize, Serialize};

/// 应用配置。
///
/// `#[serde(default)]` 保证旧版本配置文件缺少新字段时仍能反序列化成功
/// （未知字段会被忽略，因此删除字段也是安全的）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// 是否使用深色主题。
    pub dark: bool,
    /// 内容区视图模式。设计稿默认落在详细信息视图。
    pub view_mode: ViewMode,
    /// 左侧导航栏是否折叠（宽屏下持久化；窄屏浮层是临时状态，不入配置）。
    pub sidebar_collapsed: bool,
    /// 右侧预览面板是否显示。
    pub show_preview: bool,
    /// 界面密度：紧凑 / 标准 / 宽松。
    pub density: Density,
    /// 图标视图的缩略图档位：小 / 中 / 大。
    pub zoom: IconZoom,
    /// 是否显示文件扩展名。
    pub show_ext: bool,
    /// 详情表里手动拖动过的列宽。
    ///
    /// `None` = 没拖过，用断点默认值。三列各自独立，因此窄屏隐藏某列时
    /// 它的偏好仍然保留，宽屏回来时不会丢。
    pub detail_columns: ColumnWidths,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            dark: false,
            view_mode: ViewMode::Details,
            sidebar_collapsed: false,
            show_preview: true,
            density: Density::Normal,
            zoom: IconZoom::Medium,
            show_ext: true,
            detail_columns: ColumnWidths::default(),
        }
    }
}

impl Config {
    /// 配置文件路径：`%APPDATA%\pagefold\config.json`。
    ///
    /// 取不到 `APPDATA` 时返回 `None`（极简环境或 CI），此时配置读写整体静默跳过。
    pub fn path() -> Option<PathBuf> {
        let base = std::env::var_os("APPDATA")?;
        Some(PathBuf::from(base).join("pagefold").join("config.json"))
    }

    /// 读取配置，任何异常都回退到默认值。
    pub fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };

        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // 首次启动的正常路径，不需要告警。
                return Self::default();
            }
            Err(error) => {
                tracing::warn!(%error, "配置读取失败，改用默认配置");
                return Self::default();
            }
        };

        match serde_json::from_str(&text) {
            Ok(config) => config,
            Err(error) => {
                tracing::warn!(%error, "配置解析失败，改用默认配置");
                Self::default()
            }
        }
    }

    /// 写入配置。失败只记日志，不向调用方传播——保存偏好失败不该打断用户操作。
    pub fn save(&self) {
        let Some(path) = Self::path() else {
            return;
        };

        if let Some(parent) = path.parent()
            && let Err(error) = fs::create_dir_all(parent)
        {
            tracing::warn!(%error, "配置目录创建失败");
            return;
        }

        match serde_json::to_string_pretty(self) {
            Ok(text) => {
                if let Err(error) = fs::write(&path, text) {
                    tracing::warn!(%error, "配置写入失败");
                }
            }
            Err(error) => tracing::warn!(%error, "配置序列化失败"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_matches_the_design_defaults() {
        let config = Config::default();
        assert!(!config.dark, "设计稿的默认主题是浅色");
        assert_eq!(config.view_mode, ViewMode::Details, "设计稿默认列表视图");
        assert_eq!(config.density, Density::Normal);
        assert_eq!(config.zoom, IconZoom::Medium);
        assert!(config.show_preview);
        assert!(config.show_ext);
        assert!(!config.sidebar_collapsed);
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        // 模拟旧版本写下的配置：只有 dark 一个字段。
        let config: Config = serde_json::from_str(r#"{"dark":true}"#).expect("应能容忍缺字段");
        assert!(config.dark);
        assert_eq!(config.view_mode, ViewMode::Details);
        assert_eq!(config.zoom, IconZoom::Medium);
    }

    #[test]
    fn unknown_fields_are_ignored() {
        // 上一版存的是 `icon_scale`（浮点档位）。删掉它之后旧文件仍必须读得进来，
        // 否则用户升级后所有偏好会被静默重置。
        let config: Config =
            serde_json::from_str(r#"{"icon_scale":0.75,"dark":true}"#).expect("应能容忍旧字段");
        assert!(config.dark);
        assert_eq!(config.zoom, IconZoom::Medium);
    }

    #[test]
    fn round_trip_preserves_values() {
        let config = Config {
            dark: true,
            view_mode: ViewMode::Icons,
            sidebar_collapsed: true,
            show_preview: false,
            density: Density::Cozy,
            zoom: IconZoom::Large,
            show_ext: false,
            detail_columns: ColumnWidths {
                date: Some(150.0),
                kind: None,
                size: Some(96.0),
            },
        };
        let text = serde_json::to_string(&config).unwrap();
        let parsed: Config = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed, config);
    }

    #[test]
    fn configs_without_column_widths_still_load() {
        // 列宽是后加的字段：旧配置文件里没有它，必须回退到「没拖过」。
        let config: Config =
            serde_json::from_str(r#"{"dark":false,"show_ext":true}"#).expect("应能容忍缺字段");
        assert_eq!(config.detail_columns, ColumnWidths::default());
    }
}
