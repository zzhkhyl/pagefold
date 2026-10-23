//! 可自定义的快捷键：解析、目录、合并与冲突检测。
//!
//! 这一层刻意**不碰 GPUI 也不碰 Windows**：快捷键的「语义」（谁绑了什么、
//! 有没有撞车）是可以脱离界面单测的，而真正按键位注册的部分留给外壳与平台层。
//!
//! ## 两个表示形式
//!
//! 同一个组合有三种写法，必须区分清楚：
//!
//! | 用途 | 形式 | 例子 |
//! | --- | --- | --- |
//! | 存储 / 绑定 | `binding()`：小写、`-` 分隔 | `ctrl-alt-e` |
//! | 展示 | `display()`：首字母大写、` + ` 分隔 | `Ctrl + Alt + E` |
//! | 用户输入 | `parse()`：随便写 | `Ctrl+Alt+E` / `ctrl - alt - e` |

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// 一个按键组合（修饰键 + 主键），与平台无关。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyCombo {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// Win 键。它**不能**用于全局热键（`RegisterHotKey` 注册 MOD_WIN 会被系统拒绝），
    /// 但应用内快捷键可以用。
    pub win: bool,
    /// 主键，已归一化（小写）：`e` / `f5` / `,` / `delete`。
    pub key: String,
}

impl KeyCombo {
    /// 解析用户输入。认 `-`、`+`、空格三种分隔符，大小写不敏感。
    ///
    /// 返回 `None` 表示无法理解（例如只有修饰键、或空串）。
    pub fn parse(text: &str) -> Option<Self> {
        let mut ctrl = false;
        let mut alt = false;
        let mut shift = false;
        let mut win = false;
        let mut key: Option<String> = None;

        // 先按两种分隔符切成词，再逐词判定：这样 `Ctrl+Alt+E` 与 `ctrl-alt-e`
        // 走同一条路径，不必写两套解析。
        for raw in text.split(['-', '+', ' ', '\t']) {
            let token = raw.trim().to_lowercase();
            if token.is_empty() {
                continue;
            }
            match token.as_str() {
                "ctrl" | "control" => ctrl = true,
                "alt" | "option" | "opt" => alt = true,
                "shift" => shift = true,
                "win" | "super" | "meta" | "cmd" | "command" => win = true,
                _ => {
                    // 主键只能有一个；出现第二个说明输入无效（例如 `ctrl-a-b`）。
                    if key.is_some() {
                        return None;
                    }
                    key = Some(normalize_key(&token)?);
                }
            }
        }

        Some(Self {
            ctrl,
            alt,
            shift,
            win,
            key: key?,
        })
    }

    /// 存储与绑定的形式：`ctrl-alt-e`。
    ///
    /// 修饰键顺序固定（ctrl → alt → shift → win），这样「同一个组合的不同写法」
    /// 在字符串比较时也相等，配置文件不会因为顺序不同而分裂出两份。
    pub fn binding(&self) -> String {
        let mut parts: Vec<&str> = Vec::with_capacity(5);
        if self.ctrl {
            parts.push("ctrl");
        }
        if self.alt {
            parts.push("alt");
        }
        if self.shift {
            parts.push("shift");
        }
        if self.win {
            parts.push("win");
        }
        parts.push(&self.key);
        parts.join("-")
    }

    /// 展示的形式：`Ctrl + Alt + E`。
    pub fn display(&self) -> String {
        let mut parts: Vec<String> = Vec::with_capacity(5);
        if self.ctrl {
            parts.push(String::from("Ctrl"));
        }
        if self.alt {
            parts.push(String::from("Alt"));
        }
        if self.shift {
            parts.push(String::from("Shift"));
        }
        if self.win {
            parts.push(String::from("Win"));
        }
        parts.push(display_key(&self.key));
        parts.join(" + ")
    }

    /// 是否带修饰键。
    ///
    /// 「只绑一个字母」在任何界面里都是灾难：它会让该字母彻底打不出来。
    /// 设置页据此拒绝这类绑定（见 `looks_usable_as_shortcut`）。
    pub fn has_modifier(&self) -> bool {
        self.ctrl || self.alt || self.shift || self.win
    }
}

/// 主键归一化：把常见别名映射成 GPUI 认识的键名。
fn normalize_key(token: &str) -> Option<String> {
    let key = match token {
        "esc" => "escape",
        "del" => "delete",
        "return" => "enter",
        "space" | "spacebar" => "space",
        "comma" => ",",
        "dot" | "period" => ".",
        "plus" => "=",
        "pgup" | "pageup" => "pageup",
        "pgdn" | "pagedown" => "pagedown",
        other => other,
    };

    // 允许：单字符、`f1..f24`、一组具名键。其余一律拒绝——
    // 让用户写出一个 GPUI 不认识的键名，只会在运行时静默失效。
    const NAMED: &[&str] = &[
        "escape",
        "enter",
        "tab",
        "space",
        "backspace",
        "delete",
        "up",
        "down",
        "left",
        "right",
        "home",
        "end",
        "pageup",
        "pagedown",
        "insert",
    ];

    let ok = key.chars().count() == 1
        || NAMED.contains(&key)
        || key
            .strip_prefix('f')
            .is_some_and(|rest| rest.parse::<u8>().is_ok_and(|n| (1..=24).contains(&n)));

    ok.then(|| key.to_string())
}

/// 主键的展示名：单字符转大写，具名键转首字母大写。
fn display_key(key: &str) -> String {
    match key {
        "escape" => String::from("Esc"),
        "delete" => String::from("Delete"),
        "backspace" => String::from("Backspace"),
        "enter" => String::from("Enter"),
        "tab" => String::from("Tab"),
        "space" => String::from("Space"),
        "up" => String::from("↑"),
        "down" => String::from("↓"),
        "left" => String::from("←"),
        "right" => String::from("→"),
        "pageup" => String::from("PageUp"),
        "pagedown" => String::from("PageDown"),
        "home" => String::from("Home"),
        "end" => String::from("End"),
        "insert" => String::from("Insert"),
        other => other.to_uppercase(),
    }
}

/// 这个组合是否适合当快捷键。
///
/// 两条硬要求：
/// - **必须有修饰键**：单绑一个字母会把它从正常打字里抢走；
/// - **不能用 Win**：`RegisterHotKey` 注册 `MOD_WIN` 会被系统以
///   「热键已注册」拒绝（`Win+字母` 都被 shell 占着），
///   而低层钩子那条路会连带弹出开始菜单——这正是这个功能要解决的问题。
pub fn looks_usable_as_shortcut(combo: &KeyCombo) -> Result<(), &'static str> {
    if combo.win {
        return Err("Win 组合被系统占用，无法注册（请换 Ctrl / Alt 组合）");
    }
    if !combo.has_modifier() {
        return Err("必须包含 Ctrl、Alt 或 Shift");
    }
    Ok(())
}

/// 命令分组，决定设置页里的分节顺序。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Group {
    /// 标签页。
    Tabs,
    /// 导航（前进后退、地址栏）。
    Navigation,
    /// 视图切换。
    View,
    /// 选择与剪贴板。
    Selection,
    /// 编辑（重命名、删除、撤销）。
    Edit,
    /// 列表内移动。
    List,
}

impl Group {
    pub fn label(self) -> &'static str {
        match self {
            Group::Tabs => "标签页",
            Group::Navigation => "导航",
            Group::View => "视图",
            Group::Selection => "选择与剪贴板",
            Group::Edit => "编辑",
            Group::List => "列表",
        }
    }
}

/// 一个可重绑定的命令。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandSpec {
    /// 稳定标识（写进配置文件，不能随便改）。
    pub id: &'static str,
    pub label: &'static str,
    pub group: Group,
    /// 默认键位，`binding()` 形式。
    pub default_keys: &'static str,
    /// 危险动作（删除类）在界面上要标出来。
    pub danger: bool,
}

/// 全部可自定义的命令。
///
/// **顺序即展示顺序**（同组内）。这份表是唯一事实来源：
/// 启动时的绑定、设置页的列表、冲突检测都以它为准。
pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        id: "tab.new",
        label: "新建标签页",
        group: Group::Tabs,
        default_keys: "ctrl-t",
        danger: false,
    },
    CommandSpec {
        id: "tab.close",
        label: "关闭标签页",
        group: Group::Tabs,
        default_keys: "ctrl-w",
        danger: false,
    },
    CommandSpec {
        id: "tab.reopen",
        label: "恢复关闭的标签页",
        group: Group::Tabs,
        default_keys: "ctrl-shift-t",
        danger: false,
    },
    CommandSpec {
        id: "nav.back",
        label: "后退",
        group: Group::Navigation,
        default_keys: "alt-left",
        danger: false,
    },
    CommandSpec {
        id: "nav.forward",
        label: "前进",
        group: Group::Navigation,
        default_keys: "alt-right",
        danger: false,
    },
    CommandSpec {
        id: "nav.up",
        label: "返回上级",
        group: Group::Navigation,
        default_keys: "backspace",
        danger: false,
    },
    CommandSpec {
        id: "nav.address",
        label: "聚焦地址栏",
        group: Group::Navigation,
        default_keys: "ctrl-l",
        danger: false,
    },
    CommandSpec {
        id: "nav.refresh",
        label: "刷新",
        group: Group::Navigation,
        default_keys: "f5",
        danger: false,
    },
    CommandSpec {
        id: "view.details",
        label: "详细信息视图",
        group: Group::View,
        default_keys: "ctrl-1",
        danger: false,
    },
    CommandSpec {
        id: "view.icons",
        label: "图标视图",
        group: Group::View,
        default_keys: "ctrl-2",
        danger: false,
    },
    CommandSpec {
        id: "view.theme",
        label: "切换深浅色",
        group: Group::View,
        default_keys: "ctrl-3",
        danger: false,
    },
    CommandSpec {
        id: "view.settings",
        label: "打开设置",
        group: Group::View,
        default_keys: "ctrl-,",
        danger: false,
    },
    CommandSpec {
        id: "sel.all",
        label: "全选",
        group: Group::Selection,
        default_keys: "ctrl-a",
        danger: false,
    },
    CommandSpec {
        id: "sel.copy",
        label: "复制",
        group: Group::Selection,
        default_keys: "ctrl-c",
        danger: false,
    },
    CommandSpec {
        id: "sel.cut",
        label: "剪切",
        group: Group::Selection,
        default_keys: "ctrl-x",
        danger: false,
    },
    CommandSpec {
        id: "sel.paste",
        label: "粘贴",
        group: Group::Selection,
        default_keys: "ctrl-v",
        danger: false,
    },
    CommandSpec {
        id: "edit.rename",
        label: "重命名",
        group: Group::Edit,
        default_keys: "f2",
        danger: false,
    },
    CommandSpec {
        id: "edit.delete",
        label: "移到回收站",
        group: Group::Edit,
        default_keys: "delete",
        danger: true,
    },
    CommandSpec {
        id: "edit.delete_forever",
        label: "永久删除",
        group: Group::Edit,
        default_keys: "shift-delete",
        danger: true,
    },
    CommandSpec {
        id: "edit.undo",
        label: "撤销",
        group: Group::Edit,
        default_keys: "ctrl-z",
        danger: false,
    },
    CommandSpec {
        id: "list.open",
        label: "打开选中项",
        group: Group::List,
        default_keys: "enter",
        danger: false,
    },
    CommandSpec {
        id: "list.up",
        label: "上移光标",
        group: Group::List,
        default_keys: "up",
        danger: false,
    },
    CommandSpec {
        id: "list.down",
        label: "下移光标",
        group: Group::List,
        default_keys: "down",
        danger: false,
    },
    CommandSpec {
        id: "list.extend_up",
        label: "向上扩展选择",
        group: Group::List,
        default_keys: "shift-up",
        danger: false,
    },
    CommandSpec {
        id: "list.extend_down",
        label: "向下扩展选择",
        group: Group::List,
        default_keys: "shift-down",
        danger: false,
    },
];

/// 按 id 查命令。
pub fn command(id: &str) -> Option<&'static CommandSpec> {
    COMMANDS.iter().find(|command| command.id == id)
}

/// 用户对某个命令的覆盖：键位 + 是否启用。
///
/// 只存**改过**的项：没出现的命令一律用默认值，
/// 这样将来新增命令时老配置自动获得新默认值。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShortcutBinding {
    pub keys: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}

impl ShortcutBinding {
    pub fn new(keys: impl Into<String>) -> Self {
        Self {
            keys: keys.into(),
            enabled: true,
        }
    }
}

/// 全局唤醒热键的默认值。
///
/// 为什么不是 `Win+E`：`MOD_WIN` 的系统热键注册会被拒绝（shell 已占用），
/// 而用低层钩子去抢会连带弹出开始菜单——那是用户报上来的原始问题。
/// `Ctrl+Alt+E` 实测可以注册，且与 `Win+E` 的语义最接近（E = Explorer）。
pub const DEFAULT_GLOBAL_HOTKEY: &str = "ctrl-alt-e";

/// 合并后的一条快捷键（默认值 + 用户覆盖）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedShortcut {
    pub id: String,
    pub label: String,
    pub group: Group,
    pub keys: String,
    pub enabled: bool,
    /// 是否仍是默认键位（界面据此显示「默认」标记）。
    pub is_default: bool,
    pub danger: bool,
}

impl ResolvedShortcut {
    /// 展示用的键位文本。
    pub fn display_keys(&self) -> String {
        KeyCombo::parse(&self.keys)
            .map(|combo| combo.display())
            .unwrap_or_else(|| self.keys.clone())
    }
}

/// 把用户覆盖与默认值合并成一份完整清单。
pub fn resolve(overrides: &BTreeMap<String, ShortcutBinding>) -> Vec<ResolvedShortcut> {
    COMMANDS
        .iter()
        .map(|command| {
            let override_value = overrides.get(command.id);
            let keys = override_value
                .map(|binding| binding.keys.clone())
                .unwrap_or_else(|| command.default_keys.to_string());
            let enabled = override_value
                .map(|binding| binding.enabled)
                .unwrap_or(true);

            ResolvedShortcut {
                id: command.id.to_string(),
                label: command.label.to_string(),
                group: command.group,
                is_default: keys == command.default_keys,
                keys,
                enabled,
                danger: command.danger,
            }
        })
        .collect()
}

/// 一处键位冲突。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    /// 冲突的键位（展示形式）。
    pub keys: String,
    /// 撞在一起的名字（已排序，便于断言与展示）。
    pub labels: Vec<String>,
}

/// 找出**启用状态**下重复的键位。
///
/// 只有同时启用的两条才算冲突：把一条禁用掉，另一条就该正常работать——
/// 因此「禁用」本身也是一种解冲突的手段，界面要这样提示。
pub fn conflicts(items: &[ResolvedShortcut]) -> Vec<Conflict> {
    let mut by_key: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for item in items.iter().filter(|item| item.enabled) {
        // 用规范化的 binding 做键：`Ctrl+E` 与 `ctrl-e` 是同一个组合。
        let Some(combo) = KeyCombo::parse(&item.keys) else {
            continue;
        };
        by_key
            .entry(combo.binding())
            .or_default()
            .push(item.label.clone());
    }

    let mut result: Vec<Conflict> = by_key
        .into_iter()
        .filter(|(_, labels)| labels.len() > 1)
        .map(|(keys, mut labels)| {
            labels.sort();
            let display = KeyCombo::parse(&keys)
                .map(|combo| combo.display())
                .unwrap_or(keys);
            Conflict {
                keys: display,
                labels,
            }
        })
        .collect();

    // 固定顺序：先按展示文本排，界面才不会每次刷新都换行。
    result.sort_by(|left, right| left.keys.cmp(&right.keys));
    result
}

/// 找到某个键位当前的占用者（用于「这个组合已经给了 X」这类提示）。
pub fn occupant<'a>(items: &'a [ResolvedShortcut], keys: &str) -> Option<&'a ResolvedShortcut> {
    let target = KeyCombo::parse(keys)?.binding();
    items.iter().find(|item| {
        item.enabled
            && KeyCombo::parse(&item.keys)
                .is_some_and(|combo| combo.binding() == target)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overrides(pairs: &[(&str, &str, bool)]) -> BTreeMap<String, ShortcutBinding> {
        pairs
            .iter()
            .map(|(id, keys, enabled)| {
                (
                    (*id).to_string(),
                    ShortcutBinding {
                        keys: (*keys).to_string(),
                        enabled: *enabled,
                    },
                )
            })
            .collect()
    }

    #[test]
    fn parses_the_three_common_spellings_into_one_form() {
        for text in ["ctrl-alt-e", "Ctrl+Alt+E", "ctrl + alt + e", "Ctrl-Alt-E"] {
            let combo = KeyCombo::parse(text).unwrap_or_else(|| panic!("解析失败：{text}"));
            assert_eq!(combo.binding(), "ctrl-alt-e", "输入：{text}");
            assert_eq!(combo.display(), "Ctrl + Alt + E", "输入：{text}");
        }
    }

    #[test]
    fn modifier_order_is_canonical_regardless_of_input_order() {
        // 顺序不同但组合相同：存储形式必须一致，否则配置文件里会出现两份「同义」项，
        // 冲突检测也会漏判。
        let a = KeyCombo::parse("alt-ctrl-e").unwrap();
        let b = KeyCombo::parse("ctrl-alt-e").unwrap();
        assert_eq!(a.binding(), b.binding());
    }

    #[test]
    fn named_keys_are_normalized_and_unknown_ones_are_rejected() {
        assert_eq!(KeyCombo::parse("esc").unwrap().key, "escape");
        assert_eq!(KeyCombo::parse("del").unwrap().key, "delete");
        assert_eq!(KeyCombo::parse("f12").unwrap().key, "f12");
        assert_eq!(KeyCombo::parse("ctrl-,").unwrap().binding(), "ctrl-,");

        // 不认识的键名要拒绝，而不是悄悄存下去：存下去的结果是运行时静默失效。
        assert!(KeyCombo::parse("ctrl-blah").is_none());
        assert!(KeyCombo::parse("ctrl-f99").is_none());
        // 只有修饰键、没有主键：无效。
        assert!(KeyCombo::parse("ctrl-alt").is_none());
        assert!(KeyCombo::parse("").is_none());
    }

    #[test]
    fn win_combos_and_bare_letters_are_not_usable() {
        // Win 组合注册不了（系统占用），单字母会把打字抢走——两种都要拦在设置页。
        assert!(looks_usable_as_shortcut(&KeyCombo::parse("win-e").unwrap()).is_err());
        assert!(looks_usable_as_shortcut(&KeyCombo::parse("e").unwrap()).is_err());
        assert!(looks_usable_as_shortcut(&KeyCombo::parse("f5").unwrap()).is_err());
        assert!(looks_usable_as_shortcut(&KeyCombo::parse("ctrl-alt-e").unwrap()).is_ok());
    }

    #[test]
    fn resolve_fills_defaults_for_untouched_commands() {
        let items = resolve(&overrides(&[("tab.new", "ctrl-shift-n", true)]));

        let new_tab = items.iter().find(|item| item.id == "tab.new").unwrap();
        assert_eq!(new_tab.keys, "ctrl-shift-n");
        assert!(!new_tab.is_default, "改过之后不该再标默认");

        let close_tab = items.iter().find(|item| item.id == "tab.close").unwrap();
        assert_eq!(close_tab.keys, "ctrl-w");
        assert!(close_tab.is_default);
        assert!(close_tab.enabled);
    }

    #[test]
    fn conflicts_only_count_enabled_entries() {
        // 把「粘贴」改成和「复制」一样 → 两条都启用时算冲突。
        let items = resolve(&overrides(&[("sel.paste", "ctrl-c", true)]));
        let found = conflicts(&items);
        assert_eq!(found.len(), 1, "应当只有一处冲突：{found:?}");
        assert_eq!(found[0].keys, "Ctrl + C");
        assert_eq!(found[0].labels, vec!["复制", "粘贴"]);

        // 禁用其中一条 → 冲突消失（禁用本身就是一种解冲突的手段）。
        let items = resolve(&overrides(&[("sel.paste", "ctrl-c", false)]));
        assert!(conflicts(&items).is_empty());
    }

    #[test]
    fn conflict_detection_ignores_spelling_differences() {
        // 用户写成 `Ctrl+C`，默认值是 `ctrl-c`：仍然是同一个组合。
        let items = resolve(&overrides(&[("sel.paste", "Ctrl+C", true)]));
        assert_eq!(conflicts(&items).len(), 1);
    }

    #[test]
    fn occupant_finds_the_command_holding_a_combo() {
        let items = resolve(&BTreeMap::new());
        assert_eq!(
            occupant(&items, "ctrl-w").map(|item| item.id.as_str()),
            Some("tab.close")
        );
        assert!(occupant(&items, "ctrl-alt-shift-k").is_none());
    }

    #[test]
    fn every_command_id_and_default_key_is_sane() {
        // 目录本身也要守规矩：id 唯一、默认键位可解析、没有默认值互相冲突。
        let mut ids = std::collections::BTreeSet::new();
        for command in COMMANDS {
            assert!(ids.insert(command.id), "id 重复：{}", command.id);
            assert!(
                KeyCombo::parse(command.default_keys).is_some(),
                "默认键位无法解析：{} → {}",
                command.id,
                command.default_keys
            );
        }

        let items = resolve(&BTreeMap::new());
        assert!(
            conflicts(&items).is_empty(),
            "默认配置不该自带冲突：{:?}",
            conflicts(&items)
        );
    }
}
