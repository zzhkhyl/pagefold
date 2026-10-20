//! 会话持久化：把标签页的「位置与历史」带到下次启动。
//!
//! 存储位置：`%APPDATA%\pagefold\session.json`，**与 `config.json` 分开**。
//! 分开的理由不是洁癖：两者失效的后果完全不同——配置坏了只是外观回到默认，
//! 会话坏了则可能让程序启动到一个不存在的目录。分开存之后，
//! 写会话失败不会连累外观偏好，反之亦然。
//!
//! 容错原则与配置一致：文件缺失、内容损坏、版本不符、历史为空都只记 WARN 并
//! 返回 `None`，由调用方退回「单个默认标签」。一个坏掉的会话文件不应该让
//! 文件管理器打不开——那是比「回到上次位置」重要得多的性质。
//!
//! 边界（刻意不存的东西）：
//! - 扫描结果、选中集、滚动位置：重启后要么能重算，要么已经失效；
//! - 排序键与方向：目录内容可能变了，回到默认排序更可预期（见 `TabState::from_snapshot`）；
//! - 已关闭标签栈：它是「本次会话」的操作记录，跨重启复活没有对应的用户动作。

use std::fs;
use std::path::PathBuf;

use fm_core::TabSnapshot;
use serde::{Deserialize, Serialize};

/// 会话文件格式版本。
///
/// 只做「相等即接受」的判断，不尝试向前兼容：会话的语义是「恢复到上次的样子」，
/// 旧格式在新版本里恢复出来的位置未必成立，静默降级比直接回退默认标签更糟。
/// 将来若需要迁移，入口就在 [`SessionSnapshot::from_json`]。
pub const SESSION_VERSION: u32 = 1;

/// 一份完整的会话快照。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionSnapshot {
    /// 格式版本，见 [`SESSION_VERSION`]。
    pub version: u32,
    /// 按标签条顺序排列的标签快照。
    pub tabs: Vec<TabSnapshot>,
    /// 激活标签的下标。
    pub active: usize,
}

impl Default for SessionSnapshot {
    fn default() -> Self {
        Self {
            version: SESSION_VERSION,
            tabs: Vec::new(),
            active: 0,
        }
    }
}

impl SessionSnapshot {
    /// 由标签快照构造（版本号由本模块负责，调用方不用关心）。
    pub fn new(tabs: Vec<TabSnapshot>, active: usize) -> Self {
        Self {
            version: SESSION_VERSION,
            tabs,
            active,
        }
    }

    /// 会话文件路径：`%APPDATA%\pagefold\session.json`。
    ///
    /// 取不到 `APPDATA` 时返回 `None`（极简环境或 CI），此时读写整体静默跳过。
    pub fn path() -> Option<PathBuf> {
        let base = std::env::var_os("APPDATA")?;
        Some(PathBuf::from(base).join("pagefold").join("session.json"))
    }

    /// 解析并校验一段 JSON。损坏、版本不符、空会话一律返回 `None`。
    ///
    /// 抽成纯函数是为了能单测「坏数据不会 panic」这条性质——
    /// 直接测文件 IO 要么依赖 `%APPDATA%`，要么得在测试里改环境变量，
    /// 两者都会让测试变得不可靠。
    pub fn from_json(text: &str) -> Option<Self> {
        let snapshot: Self = match serde_json::from_str(text) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                tracing::warn!(%error, "会话文件解析失败，本次启动回到默认标签");
                return None;
            }
        };
        if snapshot.version != SESSION_VERSION {
            tracing::warn!(
                version = snapshot.version,
                expected = SESSION_VERSION,
                "会话文件版本不符，本次启动回到默认标签"
            );
            return None;
        }
        if snapshot.tabs.is_empty() {
            // 空会话是合法的 JSON，但恢复出来「零个标签」违反 `TabsState` 的不变量。
            tracing::warn!("会话文件不含任何标签，本次启动回到默认标签");
            return None;
        }
        Some(snapshot)
    }

    /// 序列化为 JSON 文本。
    pub fn to_json(&self) -> Option<String> {
        match serde_json::to_string_pretty(self) {
            Ok(text) => Some(text),
            Err(error) => {
                tracing::warn!(%error, "会话序列化失败，本次不保存会话");
                None
            }
        }
    }

    /// 读取会话。缺失视为首次启动（不告警），其余失败只记 WARN。
    pub fn load() -> Option<Self> {
        let path = Self::path()?;
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
            Err(error) => {
                tracing::warn!(%error, "会话文件读取失败，本次启动回到默认标签");
                return None;
            }
        };
        Self::from_json(&text)
    }

    /// 写入会话。失败只记日志——保存会话失败不该打断用户操作。
    ///
    /// **同步写**而不是去抖异步写：快照是 KB 级 JSON，与 `Config::save` 同一量级，
    /// 而异步去抖的代价是「关窗瞬间丢掉最后一次导航」这条边界。
    /// 用几 KB 的同步写换掉一条只在特定时序下出现的丢失路径，是划算的。
    pub fn save(&self) {
        let Some(path) = Self::path() else {
            return;
        };
        if let Some(parent) = path.parent()
            && let Err(error) = fs::create_dir_all(parent)
        {
            tracing::warn!(%error, "会话目录创建失败");
            return;
        }
        let Some(text) = self.to_json() else {
            return;
        };
        if let Err(error) = fs::write(&path, text) {
            tracing::warn!(%error, "会话写入失败");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fm_core::{Location, ViewMode};

    fn tab_at(path: &str) -> TabSnapshot {
        TabSnapshot {
            history: vec![Location::from(path)],
            cursor: 0,
            view_mode: ViewMode::Details,
        }
    }

    #[test]
    fn round_trip_preserves_tabs_and_active_index() {
        let session = SessionSnapshot::new(vec![tab_at(r"C:\a"), tab_at(r"C:\b")], 1);
        let text = session.to_json().expect("应可序列化");
        let parsed = SessionSnapshot::from_json(&text).expect("应可反序列化");
        assert_eq!(parsed, session);
        assert_eq!(parsed.active, 1);
    }

    #[test]
    fn corrupt_json_falls_back_instead_of_panicking() {
        assert!(SessionSnapshot::from_json("{").is_none());
        assert!(SessionSnapshot::from_json("").is_none());
        assert!(SessionSnapshot::from_json("null").is_none());
    }

    #[test]
    fn unknown_version_is_rejected() {
        let mut session = SessionSnapshot::new(vec![tab_at(r"C:\a")], 0);
        session.version = SESSION_VERSION + 1;
        let text = session.to_json().unwrap();
        assert!(
            SessionSnapshot::from_json(&text).is_none(),
            "未知版本必须回退，而不是猜着解析"
        );
    }

    #[test]
    fn empty_session_is_rejected() {
        // `TabsState` 的不变量是「至少一个标签」，恢复出零个标签会让它当场失效。
        let session = SessionSnapshot::new(Vec::new(), 0);
        assert!(SessionSnapshot::from_json(&session.to_json().unwrap()).is_none());
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        // 将来加字段时，旧文件仍应能读进来（字段级 default 的语义）。
        let parsed = SessionSnapshot::from_json(
            r#"{"version":1,"tabs":[{"history":["Computer"],"cursor":0,"view_mode":"Icons"}],"active":0}"#,
        )
        .expect("缺字段应回退到默认值");
        assert_eq!(parsed.tabs[0].history, vec![Location::Computer]);
        assert_eq!(parsed.tabs[0].view_mode, ViewMode::Icons);
    }
}
