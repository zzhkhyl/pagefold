//! 快捷键绑定的装配与应用。
//!
//! 这个模块是「用户配置」与「GPUI 键位表」之间唯一的翻译层：
//!
//! ```text
//! Config.shortcuts ──resolve──▶ 每条命令最终键位
//!                              │
//!                              ├─ 命令 id ──match──▶ 具体 Action 类型（编译期确定）
//!                              ▼
//!                        cx.bind_keys(...)
//! ```
//!
//! ## 为什么是 `match` 而不是 `Box<dyn Action>`
//!
//! GPUI 的 `KeyBinding::new` 要求 `A: Action`（`Sized`），`Box<dyn Action>` 不满足；
//! 想在运行期动态拼装只能走 `KeyBinding::load`，那还要自己提供 `PlatformKeyboardMapper`。
//! 用一条 `match` 把「id → 具体类型」写死，代价是加命令时要记得加一行，
//! 收益是类型安全、没有运行期反射——并且有测试盯着「每条命令都接上了」。

use fm_core::KeyCombo;
use gpui::{App, KeyBinding, Keystroke};
use fm_services::config::Config;

use crate::KEY_CONTEXT;
use crate::actions::{
    CloseTab, CopyItems, CutItems, DeleteItems, DeletePermanently, ExtendCursorDown, ExtendCursorUp,
    FocusAddressBar, GoBack, GoForward, GoUp, MoveCursorDown, MoveCursorUp, NewTab, OpenItem,
    OpenSettings, PasteItems, Refresh, RenameItem, ReopenClosedTab, SelectAll, SetViewDetails,
    SetViewIcons, ToggleTheme, Undo,
};

/// 一条命令的绑定的键位字符串。
///
/// 只做合法性过滤，不做语义判断（禁用项已经在外面被滤掉了）。
fn usable(item: &fm_core::ResolvedShortcut) -> bool {
    // 手改过配置文件可能留下 `ctrl-blah` 这类无法解析的键位：
    // 直接交给 `KeyBinding::new` 会 **panic**，所以先挡掉。
    KeyCombo::parse(&item.keys).is_some()
}

/// 把配置翻译成 GPUI 键位表。
///
/// 返回的列表可以直接喂给 `cx.bind_keys`。禁用项与无法解析的项会被跳过。
pub(crate) fn bindings(config: &Config) -> Vec<KeyBinding> {
    let items = fm_core::resolve(&config.shortcuts);

    items
        .iter()
        .filter(|item| item.enabled)
        .filter(|item| usable(item))
        .filter_map(|item| {
            let keys = item.keys.as_str();
            // 每条命令的 Action 类型在这里固定下来：改配置只改键位，不改行为。
            let binding = match item.id.as_str() {
                "tab.new" => KeyBinding::new(keys, NewTab, Some(KEY_CONTEXT)),
                "tab.close" => KeyBinding::new(keys, CloseTab, Some(KEY_CONTEXT)),
                "tab.reopen" => KeyBinding::new(keys, ReopenClosedTab, Some(KEY_CONTEXT)),
                "nav.back" => KeyBinding::new(keys, GoBack, Some(KEY_CONTEXT)),
                "nav.forward" => KeyBinding::new(keys, GoForward, Some(KEY_CONTEXT)),
                "nav.up" => KeyBinding::new(keys, GoUp, Some(KEY_CONTEXT)),
                "nav.address" => KeyBinding::new(keys, FocusAddressBar, Some(KEY_CONTEXT)),
                "nav.refresh" => KeyBinding::new(keys, Refresh, Some(KEY_CONTEXT)),
                "view.details" => KeyBinding::new(keys, SetViewDetails, Some(KEY_CONTEXT)),
                "view.icons" => KeyBinding::new(keys, SetViewIcons, Some(KEY_CONTEXT)),
                "view.theme" => KeyBinding::new(keys, ToggleTheme, Some(KEY_CONTEXT)),
                "view.settings" => KeyBinding::new(keys, OpenSettings, Some(KEY_CONTEXT)),
                "sel.all" => KeyBinding::new(keys, SelectAll, Some(KEY_CONTEXT)),
                "sel.copy" => KeyBinding::new(keys, CopyItems, Some(KEY_CONTEXT)),
                "sel.cut" => KeyBinding::new(keys, CutItems, Some(KEY_CONTEXT)),
                "sel.paste" => KeyBinding::new(keys, PasteItems, Some(KEY_CONTEXT)),
                "edit.rename" => KeyBinding::new(keys, RenameItem, Some(KEY_CONTEXT)),
                "edit.delete" => KeyBinding::new(keys, DeleteItems, Some(KEY_CONTEXT)),
                "edit.delete_forever" => KeyBinding::new(keys, DeletePermanently, Some(KEY_CONTEXT)),
                "edit.undo" => KeyBinding::new(keys, Undo, Some(KEY_CONTEXT)),
                "list.open" => KeyBinding::new(keys, OpenItem, Some(KEY_CONTEXT)),
                "list.up" => KeyBinding::new(keys, MoveCursorUp, Some(KEY_CONTEXT)),
                "list.down" => KeyBinding::new(keys, MoveCursorDown, Some(KEY_CONTEXT)),
                "list.extend_up" => KeyBinding::new(keys, ExtendCursorUp, Some(KEY_CONTEXT)),
                "list.extend_down" => KeyBinding::new(keys, ExtendCursorDown, Some(KEY_CONTEXT)),
                other => {
                    // 新增了命令但忘了在这里接上：留一条日志，别静默失效。
                    tracing::warn!(id = other, "快捷键目录里有未接线的命令");
                    return None;
                }
            };
            Some(binding)
        })
        .collect()
}

/// 重建整张键位表。
///
/// ## 为什么必须先清空
///
/// `bind_keys` 是**累加**的，直接叠加会让改键之后旧键位仍然生效，
/// 表现为「改了这个键，结果两个键都能用」。
///
/// ## 为什么必须再补回 `base`
///
/// `clear_key_bindings()` 清的是**整张表**，包括组件库在 `gpui_component::init`
/// 里注册的那些（输入框的 `backspace` / `enter` / `ctrl-a` / 方向键……）。
/// 直接清掉而不再补回来，后果是**所有输入框失去退格、回车、全选**——
/// 只留下「打字」（打字走平台的文本输入通道，不经过键位表），
/// 于是看起来就是「能输但删不掉」。
///
/// 因此调用方要在**第一次应用之前**把库自带的绑定抄一份存起来（见 `ShellView`），
/// 之后每次重建都：清空 → 补回库绑定 → 绑定我们的。
pub(crate) fn apply(cx: &mut App, config: &Config, base: &[KeyBinding]) {
    cx.clear_key_bindings();
    cx.bind_keys(base.to_vec());
    cx.bind_keys(bindings(config));
}

/// 抄录当前键位表（用于取得组件库自带的绑定）。
///
/// 只能在**我们还没有绑定过任何东西**时调用，抄到的才是纯库绑定。
pub(crate) fn snapshot(cx: &App) -> Vec<KeyBinding> {
    cx.key_bindings().borrow().bindings().cloned().collect()
}

/// 把一次真实按键翻译成配置能存的形式。
///
/// 返回 `None` 表示这个按键不能被当作快捷键：
/// - 只按下了修饰键（`Ctrl` 本身）；
/// - 主键不在我们的白名单里（例如某些输入法键）。
pub(crate) fn from_keystroke(keystroke: &Keystroke) -> Option<KeyCombo> {
    let mut parts: Vec<&str> = Vec::with_capacity(5);
    let modifiers = &keystroke.modifiers;
    if modifiers.control {
        parts.push("ctrl");
    }
    if modifiers.alt {
        parts.push("alt");
    }
    if modifiers.shift {
        parts.push("shift");
    }
    if modifiers.platform {
        parts.push("win");
    }

    let key = keystroke.key.to_lowercase();
    // 纯修饰键：GPUI 也会把它们作为 key 报上来，不能当成完整组合。
    const MODIFIER_KEYS: &[&str] = &[
        "ctrl", "control", "alt", "shift", "win", "super", "cmd", "command", "fn", "meta",
    ];
    if MODIFIER_KEYS.contains(&key.as_str()) || key.is_empty() {
        return None;
    }
    parts.push(&key);

    // 复用 core 的解析：白名单、别名、规范化都在那一处，避免两套规则打架。
    KeyCombo::parse(&parts.join("-"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fm_core::shortcut::COMMANDS;

    #[test]
    fn every_catalogued_command_is_wired_up() {
        // 这条测试守的是「加了命令却没接线」：那种 bug 的表现是
        // 「设置页里改了键，按下去没反应」，而且很难联想到是漏了一条 match。
        let config = Config::default();
        let bindings = bindings(&config);
        assert_eq!(
            bindings.len(),
            COMMANDS.len(),
            "目录里有 {} 条命令，但只接上了 {} 条",
            COMMANDS.len(),
            bindings.len()
        );
    }

    #[test]
    fn disabled_commands_are_not_bound() {
        let mut config = Config::default();
        config.shortcuts.insert(
            String::from("tab.new"),
            fm_core::ShortcutBinding {
                keys: String::from("ctrl-t"),
                enabled: false,
            },
        );

        let bindings = bindings(&config);
        assert_eq!(bindings.len(), COMMANDS.len() - 1, "禁用项不该出现在键位表里");
    }

    #[test]
    fn hand_edited_garbage_is_skipped_instead_of_panicking() {
        // `KeyBinding::new` 在解析失败时会 panic；配置文件是用户可编辑的，
        // 因此这里必须先过滤，否则一个手滑的字符就能让程序起不来。
        let mut config = Config::default();
        config.shortcuts.insert(
            String::from("tab.close"),
            fm_core::ShortcutBinding {
                keys: String::from("ctrl-blah"),
                enabled: true,
            },
        );

        let bindings = bindings(&config);
        assert_eq!(bindings.len(), COMMANDS.len() - 1);
    }

    #[test]
    fn keystrokes_round_trip_through_the_parser() {
        let combo = KeyCombo::parse("ctrl-shift-n").unwrap();
        assert_eq!(combo.binding(), "ctrl-shift-n");
        assert_eq!(combo.display(), "Ctrl + Shift + N");
    }
}
