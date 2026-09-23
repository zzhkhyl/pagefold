//! 配置持久化。
//!
//! 存储位置：`%APPDATA%\files-gpui\config.json`。
//!
//! 容错原则：配置是「锦上添花」而不是启动前提。
//! 文件缺失、内容损坏、字段新增/删除、写盘失败，都只记 WARN 日志并回退到默认值——
//! 一个坏掉的配置文件不应该让文件管理器打不开。

use std::fs;
use std::path::PathBuf;

use fm_core::ViewMode;
use serde::{Deserialize, Serialize};

/// 应用配置。
///
/// `#[serde(default)]` 保证旧版本配置文件缺少新字段时仍能反序列化成功。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// 是否使用深色主题。
    pub dark: bool,
    /// 内容区视图模式。
    pub view_mode: ViewMode,
    /// 左侧导航栏是否折叠。
    pub sidebar_collapsed: bool,
    /// 图标视图的缩放级别，取值 `0.0..=1.0`（映射到 UI 上的滑块）。
    pub icon_scale: f32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            dark: false,
            view_mode: ViewMode::Icons,
            sidebar_collapsed: false,
            icon_scale: 0.5,
        }
    }
}

impl Config {
    /// 配置文件路径：`%APPDATA%\files-gpui\config.json`。
    ///
    /// 取不到 `APPDATA` 时返回 `None`（极简环境或 CI），此时配置读写整体静默跳过。
    pub fn path() -> Option<PathBuf> {
        let base = std::env::var_os("APPDATA")?;
        Some(PathBuf::from(base).join("files-gpui").join("config.json"))
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
    fn default_config_is_light_theme_and_icons_view() {
        let config = Config::default();
        assert!(!config.dark);
        assert_eq!(config.view_mode, ViewMode::Icons);
        assert!(!config.sidebar_collapsed);
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        // 模拟旧版本写下的配置：只有 dark 一个字段。
        let config: Config = serde_json::from_str(r#"{"dark":true}"#).expect("应能容忍缺字段");
        assert!(config.dark);
        assert_eq!(config.view_mode, ViewMode::Icons);
        assert_eq!(config.icon_scale, 0.5);
    }

    #[test]
    fn round_trip_preserves_values() {
        let config = Config {
            dark: true,
            view_mode: ViewMode::Details,
            sidebar_collapsed: true,
            icon_scale: 0.75,
        };
        let text = serde_json::to_string(&config).unwrap();
        let parsed: Config = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed, config);
    }
}
