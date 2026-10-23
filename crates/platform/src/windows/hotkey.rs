//! 全局热键：解析、注册、可用性探测与「通知宿主重新读取」。
//!
//! ## 为什么是 `RegisterHotKey` 而不是键盘钩子
//!
//! 用低层键盘钩子抢 `Win+E` 会带来一个没法绕开的副作用：把 `E` 吞掉之后，
//! 系统只看到「单独按了一下 Win」，松开就弹开始菜单（Win11 会先铺一层暗色遮罩，
//! 用户看到的是屏幕暗一下）。
//!
//! 而压制开始菜单的两条路都被证伪：
//!
//! - 吞掉 Win 抬起 → 系统收不到释放，之后所有按键都变成 `Win+字母`（键盘失灵）；
//! - 补发一个按键 → 实测无效，Windows 会丢弃合成的 Win 事件，假事件进不了组合判定。
//!
//! `RegisterHotKey` 是从根上避开这件事：由系统来分发这个组合，
//! 它本来就不会被当成「单独按了 Win」。代价是**只能注册没有被占用的组合**——
//! `Win+字母` 全被 shell 占着，所以默认值用 `Ctrl+Alt+E`。
//!
//! ## 谁在读配置
//!
//! 主程序把要注册的组合写进 `hotkey.txt`，宿主启动时读它；
//! 用户改了之后主程序再写一次、并发一条 [`WM_HOST_RELOAD`] 让宿主重注册。
//! 用文件而不是把字符串塞进 `PostMessage`，是为了不引入第二套序列化。

use std::path::PathBuf;

use anyhow::Context;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, RegisterHotKey,
    UnregisterHotKey,
};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW};
use windows::core::{PCWSTR, w};

/// 宿主注册全局热键时使用的 id。
pub const HOTKEY_ID: i32 = 1;

/// 宿主的窗口类名。主程序靠它找到宿主并发消息。
pub const HOST_CLASS: PCWSTR = w!("PagefoldHotkeyHost");

/// 让宿主重新读取 `hotkey.txt` 并重注册。
pub const WM_HOST_RELOAD: u32 = 0x8000 + 3;

/// 探测可用性时用的 id 段（与宿主那个 1 分开，避免自撞）。
const PROBE_ID: i32 = 0x7000;

/// 把 `ctrl-alt-e` 解析成 `(修饰键掩码, 虚拟键码)`。
///
/// 返回 `None` 表示这个组合无法用 `RegisterHotKey` 表达（键名不认识）。
pub fn mods_and_vk(keys: &str) -> Option<(u32, u32)> {
    let combo = fm_core::KeyCombo::parse(keys)?;

    let mut mods = MOD_NOREPEAT.0;
    if combo.ctrl {
        mods |= MOD_CONTROL.0;
    }
    if combo.alt {
        mods |= MOD_ALT.0;
    }
    if combo.shift {
        mods |= MOD_SHIFT.0;
    }
    if combo.win {
        mods |= MOD_WIN.0;
    }

    Some((mods, vk_for(&combo.key)?))
}

/// 键名 → 虚拟键码。
///
/// 只覆盖 `fm_core::KeyCombo` 允许的那些键：那边已经做过归一化（`esc` → `escape`、
/// `del` → `delete`、`f1..f24` 小写），这里不必再兜一遍别名。
fn vk_for(key: &str) -> Option<u32> {
    let mut chars = key.chars();
    if let (Some(ch), None) = (chars.next(), chars.next()) {
        return match ch {
            'a'..='z' => Some(u32::from(ch.to_ascii_uppercase())),
            '0'..='9' => Some(u32::from(ch)),
            // OEM 键：与物理键位无关，按《Virtual-Key Codes》的固定值。
            ',' => Some(0xBC),
            '.' => Some(0xBE),
            '/' => Some(0xBF),
            ';' => Some(0xBA),
            '\'' => Some(0xDE),
            '[' => Some(0xDB),
            ']' => Some(0xDD),
            '\\' => Some(0xDC),
            '-' => Some(0xBD),
            '=' => Some(0xBB),
            '`' => Some(0xC0),
            _ => None,
        };
    }

    if let Some(rest) = key.strip_prefix('f') {
        if let Ok(number) = rest.parse::<u32>()
            && (1..=24).contains(&number)
        {
            // F1 = 0x70。
            return Some(0x70 + number - 1);
        }
        return None;
    }

    Some(match key {
        "escape" => 0x1B,
        "enter" => 0x0D,
        "tab" => 0x09,
        "space" => 0x20,
        "backspace" => 0x08,
        "delete" => 0x2E,
        "insert" => 0x2D,
        "home" => 0x24,
        "end" => 0x23,
        "pageup" => 0x21,
        "pagedown" => 0x22,
        "up" => 0x26,
        "down" => 0x28,
        "left" => 0x25,
        "right" => 0x27,
        _ => return None,
    })
}

/// 在指定窗口上注册全局热键。
pub fn register(window: HWND, keys: &str) -> anyhow::Result<()> {
    let (mods, vk) = mods_and_vk(keys)
        .with_context(|| format!("无法把「{keys}」解析成系统热键"))?;

    // SAFETY: `window` 由调用方保证在注册期间有效；失败时按错误码返回。
    unsafe { RegisterHotKey(Some(window), HOTKEY_ID, HOT_KEY_MODIFIERS(mods), vk) }
        .with_context(|| format!("注册全局热键「{keys}」失败（可能已被其它程序占用）"))
}

/// 注销当前窗口上的全局热键（没注册过也无妨）。
pub fn unregister(window: HWND) {
    unsafe {
        let _ = UnregisterHotKey(Some(window), HOTKEY_ID);
    }
}

/// 这个组合现在能不能被我们注册？
///
/// 用一次「注册 → 立刻注销」来探测。注意：**如果宿主已经占着这个组合**，
/// 探测会失败（错误码 1409），调用方需要把这种情况当作「自己的」处理
/// （见 `ShellView::assign_global_hotkey`）。
pub fn probe(keys: &str) -> Result<(), String> {
    let Some((mods, vk)) = mods_and_vk(keys) else {
        return Err(String::from("这个组合无法注册为系统热键"));
    };

    // SAFETY: 传 `None` 表示把热键消息投到调用线程的队列；探测完立刻注销。
    let registered = unsafe {
        RegisterHotKey(None, PROBE_ID, HOT_KEY_MODIFIERS(mods), vk)
    };
    match registered {
        Ok(()) => {
            unsafe {
                let _ = UnregisterHotKey(None, PROBE_ID);
            }
            Ok(())
        }
        Err(error) => {
            let code = error.code().0 as u32 & 0xffff;
            Err(match code {
                // ERROR_HOTKEY_ALREADY_REGISTERED
                1409 => String::from("已被其它程序占用"),
                _ => format!("系统拒绝了这个组合（错误码 {code}）"),
            })
        }
    }
}

/// 宿主与主程序之间传递热键的约定文件。
pub fn spec_file() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("pagefold").join("hotkey.txt")
}

/// 写入要注册的组合。`None` = 不注册任何全局热键。
pub fn write_spec(keys: Option<&str>) -> anyhow::Result<()> {
    let path = spec_file();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("无法创建配置目录「{}」", parent.display()))?;
    }
    std::fs::write(&path, keys.unwrap_or_default())
        .with_context(|| format!("无法写入「{}」", path.display()))
}

/// 读取要注册的组合。空文件或文件缺失都表示「不注册」。
pub fn read_spec() -> Option<String> {
    let text = std::fs::read_to_string(spec_file()).ok()?;
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// 通知已运行的宿主重新读取 [`spec_file`]。
///
/// 返回 `false` 表示宿主没在运行——那不是错误：热键只在宿主活着时才有效。
pub fn notify_host() -> bool {
    // SAFETY: `FindWindowW` 只查窗口，不持有引用；找不到会返回 `Err`。
    let Ok(window) = (unsafe { FindWindowW(HOST_CLASS, PCWSTR::null()) }) else {
        return false;
    };
    unsafe { PostMessageW(Some(window), WM_HOST_RELOAD, Default::default(), Default::default()) }
        .is_ok()
}

/// 组合是否可用（不涉及探测，纯语法判断）。
pub fn looks_valid(keys: &str) -> bool {
    mods_and_vk(keys).is_some()
}

/// 注册失败时的提一句人话（宿主日志与设置页共用）。
pub fn describe_failure(keys: &str) -> String {
    if !looks_valid(keys) {
        return format!("「{keys}」不是有效的组合");
    }
    match probe(keys) {
        Err(reason) => format!("「{keys}」{reason}"),
        Ok(()) => format!("「{keys}」注册失败"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_default_global_hotkey() {
        let (mods, vk) = mods_and_vk(fm_core::shortcut::DEFAULT_GLOBAL_HOTKEY).expect("默认值应当可解析");
        assert_ne!(mods & MOD_CONTROL.0, 0, "应当带 Ctrl");
        assert_ne!(mods & MOD_ALT.0, 0, "应当带 Alt");
        assert_eq!(mods & MOD_SHIFT.0, 0, "默认值不该带 Shift");
        assert_eq!(vk, u32::from(b'E'), "E 的虚拟键码就是字母本身");
        assert_ne!(mods & MOD_NOREPEAT.0, 0, "必须带 NOREPEAT，否则长按会反复触发");
    }

    #[test]
    fn maps_named_and_function_keys() {
        assert_eq!(vk_for("f1"), Some(0x70));
        assert_eq!(vk_for("f24"), Some(0x87));
        assert_eq!(vk_for("f25"), None, "F25 不存在");
        assert_eq!(vk_for("delete"), Some(0x2E));
        assert_eq!(vk_for("escape"), Some(0x1B));
        assert_eq!(vk_for(","), Some(0xBC));
        assert_eq!(vk_for("5"), Some(0x35));
        // 未知名字要明确拒绝，而不是猜一个键——猜出来的键会静默注册成别的组合。
        assert_eq!(vk_for("blah"), None);
    }

    #[test]
    fn win_combos_are_parsable_but_known_to_be_taken() {
        // 解析得出来（语法没问题），但系统拒绝注册——这正是设置页要提示用户的原因。
        assert!(mods_and_vk("win-e").is_some());
        let result = probe("win-e");
        assert!(result.is_err(), "Win+E 应当被系统占用");
        assert!(
            result.unwrap_err().contains("占用"),
            "提示要说清「被占用」，而不是一句系统错误码"
        );
    }

    #[test]
    fn the_registration_path_works_on_a_combo_nobody_wants() {
        // 验证「注册 → 注销」这条链路真的通。
        //
        // 刻意**不去探测默认值**：热键宿主可能正跑着并占着 `Ctrl+Alt+E`，
        // 那样这条测试的结果就取决于开发机上有没有开宿主——那不是我们要断言的东西。
        // 用一个几乎不可能被占用的组合，才能稳定测到「注册本身可行」。
        let free = "ctrl-alt-shift-f24";
        probe(free).unwrap_or_else(|reason| panic!("空闲组合应当可以注册：{reason}"));

        // 注册完必须真的被释放，否则下一次（或别的程序）就用不了这个组合。
        probe(free).unwrap_or_else(|reason| panic!("探测后应当已注销：{reason}"));
    }
}
