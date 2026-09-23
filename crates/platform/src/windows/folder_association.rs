//! 文件夹接管：右键菜单项 + 「文件夹默认打开程序」。
//!
//! ## 为什么必须改注册表
//!
//! Windows **没有「默认文件管理器」这个可设置的项**（设置 → 默认应用里没有这一栏），
//! 也没有任何受支持的 API。唯一可行的办法是覆盖 shell 的文件夹打开处理器，
//! 这是 Files / Total Commander / Directory Opus 这类工具的通行做法。
//!
//! ## 全部写在 HKCU，不碰 HKLM
//!
//! `HKEY_CURRENT_USER\Software\Classes` 是**叠加**在机器级 `HKLM\Software\Classes`
//! 之上的一层，因此：
//!
//! * 不需要管理员权限；
//! * **还原只需删掉我们写的键** —— HKLM 里的原始值原封不动，行为自动恢复。
//!   这是本模块最重要的安全性质：不依赖「备份回写」这一步是否正确。
//!
//! 唯一的例外是用户此前已在 HKCU 里自定义过同一个键。那种情况下覆盖前会把原值
//! （**含值的类型**）备份到 [`OWN_KEY`]，还原时优先回写备份。
//!
//! ## 每个 progid 要写两条，缺一不可
//!
//! 1. `...\shell` 的**默认值** = `open` —— 双击文件夹时用的正是这个「默认动词」。
//!    工厂状态下它是**未设置**的，此时双击由资源管理器内部处理，根本不走 shell 动词。
//! 2. `...\shell\open\command` 的默认值 = 我们的命令行。
//!
//! 只写第 2 条的话，**右键菜单会生效**（菜单是显式枚举动词），但**双击毫无变化** ——
//! 这正是「右键接管了、双击还是资源管理器」的成因。这个坑踩过一次。
//!
//! 需要覆盖三个 progid：`Folder`（文件夹类类型的公共父级）、`Directory`（文件系统文件夹）、
//! `Drive`（盘符，「此电脑」或桌面上双击 `C:` 走它）。
//!
//! ## 改完必须广播
//!
//! 关联变更后要调用 `SHChangeNotify(SHCNE_ASSOCCHANGED, …)`，否则已经在运行的
//! 资源管理器会继续用缓存里的旧处理程序，必须重启 `explorer.exe` 才生效。
//!
//! ## 影响范围（使用前务必了解）
//!
//! 覆盖 `Folder\shell\open\command` 会影响**所有经由 shell 发起的文件夹打开**：
//! 桌面双击、其他程序调 `ShellExecute`（浏览器的「在文件夹中显示」等）。
//!
//! 它**不会**影响：
//!
//! * 资源管理器窗口内部的双击导航 —— 那是 Explorer 的内部行为，不走 shell 动词；
//! * 任务栏 / 开始菜单里的「文件资源管理器」按钮 —— 它直接启动 `explorer.exe`；
//! * 「此电脑」「回收站」—— 它们走 CLSID 命名空间。
//!
//! 要做到上面这些只能改 `Winlogon\Shell`，让本程序充当整个桌面外壳。
//! 那超出本项目范围：我们没有任务栏、桌面图标层与开始菜单。
//!
//! ## 足控与安全网
//!
//! 覆盖生效后，**若本程序被删除或移动，双击文件夹将打不开**，只能靠 `regedit` 修复。
//! 为此安装时会在程序目录旁边写一个 `restore-explorer.bat`：
//! 即使程序本体已经损坏，双击它也能恢复。

use std::path::Path;

use anyhow::Context as _;
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, ERROR_SUCCESS};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ,
    REG_VALUE_TYPE, RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegDeleteValueW, RegOpenKeyExW,
    RegQueryValueExW, RegSetValueExW,
};
use windows::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
use windows::core::PCWSTR;

/// 我们自己的登记处：存放原值备份。
const OWN_KEY: &str = r"Software\Files\FolderIntegration";

/// 文件夹右键菜单项（作用于文件夹本身）。`%1` 是被选中的那个文件夹。
const CONTEXT_MENU_DIRECTORY: &str = r"Software\Classes\Directory\shell\Files";
/// 上者的命令子键。
///
/// 动词键的**默认值是菜单显示文字**，命令在 `\command` 子键的默认值里。
/// 这两者混淆过一次（状态判定读成显示文字，于是永远报「未安装」），
/// 因此把它们作为一对常量放在一起，并由 [`CONTEXT_MENU_KEYS`] 统一使用。
const CONTEXT_MENU_DIRECTORY_COMMAND: &str = r"Software\Classes\Directory\shell\Files\command";
/// 文件夹右键菜单项（作用于文件夹内的空白处）。此时没有选中项，
/// 路径来自 `%V`（当前目录），用 `%1` 会拿到空串。
const CONTEXT_MENU_BACKGROUND: &str = r"Software\Classes\Directory\Background\shell\Files";
/// 上者的命令子键。
const CONTEXT_MENU_BACKGROUND_COMMAND: &str =
    r"Software\Classes\Directory\Background\shell\Files\command";
/// 默认动词的名字。
///
/// **这是本功能成败的关键一步。** 工厂状态下 `...\shell` 的默认值是**未设置**的，
/// 此时双击文件夹不会去走 shell 动词（由资源管理器内部处理）；必须把它显式指成 `open`，
/// 双击才会真正去调用 `...\shell\open\command`。
///
/// 只写 `...\shell\open\command` 而不写默认动词，症状正是「右键菜单生效了，
/// 但双击文件夹仍然打开资源管理器」—— 因为双击用的是默认动词，而不是 `open` 这个动词。
const DEFAULT_VERB: &str = "open";

/// 需要接管「默认打开程序」的 progid。
///
/// 三个都要写，缺一个就有一类入口漏掉：
///
/// * `Folder` —— 文件夹类类型的公共父级，`Directory` 与 `Drive` 都从它派生；
/// * `Directory` —— 文件系统文件夹（桌面双击、其他程序调 `ShellExecute` 走它）；
/// * `Drive` —— 盘符（「此电脑」里或桌面上双击 `C:` 走它）。
#[derive(Clone, Copy)]
struct HandlerTarget {
    /// progid 根键。
    progid: &'static str,
    /// 备份值名里的短标识，拼成 `Backup.<id>.Open` 之类。
    id: &'static str,
}

const HANDLER_TARGETS: [HandlerTarget; 3] = [
    HandlerTarget {
        progid: r"Software\Classes\Folder",
        id: "Folder",
    },
    HandlerTarget {
        progid: r"Software\Classes\Directory",
        id: "Directory",
    },
    HandlerTarget {
        progid: r"Software\Classes\Drive",
        id: "Drive",
    },
];

impl HandlerTarget {
    /// `...\shell` —— 它的**默认值是默认动词名**，不是命令。
    fn shell_key(&self) -> String {
        format!(r"{}\shell", self.progid)
    }

    /// `...\shell\open\command` —— 命令写在这里。
    fn command_key(&self) -> String {
        format!(r"{}\shell\open\command", self.progid)
    }

    fn backup_verb(&self) -> String {
        format!("Backup.{}.Verb", self.id)
    }

    fn backup_verb_type(&self) -> String {
        format!("Backup.{}.VerbType", self.id)
    }

    fn backup_open(&self) -> String {
        format!("Backup.{}.Open", self.id)
    }

    fn backup_open_type(&self) -> String {
        format!("Backup.{}.OpenType", self.id)
    }
}

/// 安装标记：存在即表示我们**曾经**覆盖过默认打开程序。
///
/// 它唯一的用途是让重复安装不去备份我们自己写的命令。没有它，
/// 第二次安装会把我们的命令当作「用户原值」存进备份，
/// 之后还原就变成把这个命令又写回去 —— 表现是「点了还原但文件夹还是打不开」，
/// 而且完全无声，因为每一步都返回成功。
const INSTALLED_MARKER: &str = "DefaultHandlerInstalled";

/// 还原脚本的文件名（写在程序目录旁边）。
const RESTORE_SCRIPT: &str = "restore-explorer.bat";

/// 右键菜单项显示的文字。
const CONTEXT_MENU_LABEL: &str = "用 Files 打开";

/// 两处右键菜单项：(动词键, 命令子键, 位置参数)。
///
/// 同样用表驱动：安装、状态判定、卸载三处都遍历它，
/// 新增一个挂载点时不可能漏掉其中任何一处。
const CONTEXT_MENU_KEYS: [(&str, &str, &str); 2] = [
    (
        CONTEXT_MENU_DIRECTORY,
        CONTEXT_MENU_DIRECTORY_COMMAND,
        SELECTED_ARGUMENT,
    ),
    (
        CONTEXT_MENU_BACKGROUND,
        CONTEXT_MENU_BACKGROUND_COMMAND,
        BACKGROUND_ARGUMENT,
    ),
];

/// 文件夹接管的当前状态。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IntegrationState {
    /// 右键菜单项是否已安装。
    pub context_menu: bool,
    /// 文件夹的默认打开程序是否已指向本程序。
    pub default_handler: bool,
}

impl IntegrationState {
    /// 是否处于「完全接管」状态。
    pub fn is_fully_installed(&self) -> bool {
        self.context_menu && self.default_handler
    }
}

/// 读取当前状态。
///
/// 每次都从注册表现读、不缓存：注册表可能被外部工具改动，
/// 而调用方（托盘菜单）本来就在重建菜单，顺手读一次最准确。
pub fn state(exe_path: &Path) -> IntegrationState {
    IntegrationState {
        // 比较的是**命令子键**而不是动词键：动词键的默认值是菜单显示文字，
        // 读错会得到「注册表明明写了，状态却报未安装」。
        context_menu: CONTEXT_MENU_KEYS
            .iter()
            .all(|(_, command, argument)| is_our_command(command, &shell_command(exe_path, argument))),
        // 默认动词也必须是我们写的。只写命令而不写默认动词时，双击文件夹
        // 仍然不会走 shell 动词 —— 接管等于没生效，这正是「右键生效但双击无效」的成因。
        default_handler: HANDLER_TARGETS.iter().all(|target| {
            is_our_command(
                &target.command_key(),
                &shell_command(exe_path, SELECTED_ARGUMENT),
            ) && reads_verb(&target.shell_key()).as_deref() == Some(DEFAULT_VERB)
        }),
    }
}

/// 安装或卸载右键菜单项。
pub fn set_context_menu(exe_path: &Path, enabled: bool) -> anyhow::Result<()> {
    if enabled {
        for (verb, command, argument) in CONTEXT_MENU_KEYS {
            // 动词键的默认值是**菜单显示文字**，命令写在 `\command` 子键里。
            Key::open(verb, true)?.write_string(None, CONTEXT_MENU_LABEL, REG_SZ)?;
            // 不写 `Icon`：本程序还没有图标资源，写了只会指向一个不存在的图标索引。
            Key::open(command, true)?
                .write_string(None, &shell_command(exe_path, argument), REG_SZ)?;
        }
        tracing::info!("已添加文件夹右键菜单项");
    } else {
        // 删动词键即可连带删掉它的命令子键。
        for (verb, ..) in CONTEXT_MENU_KEYS {
            delete_tree(verb)?;
        }
        tracing::info!("已移除文件夹右键菜单项");
    }
    Ok(())
}

/// 安装或卸载「文件夹默认打开程序」。
pub fn set_default_handler(exe_path: &Path, enabled: bool) -> anyhow::Result<()> {
    let command = shell_command(exe_path, SELECTED_ARGUMENT);

    for target in HANDLER_TARGETS {
        if enabled {
            back_up_existing(&target)?;

            // 两条都要写，缺任何一条接管都不生效：
            // 默认动词决定「双击时调用哪个动词」，命令决定「这个动词执行什么」。
            // 只写命令的话，`...\shell` 的默认值仍未设置，双击走的还是资源管理器的内部处理。
            Key::open(&target.shell_key(), true)?.write_string(None, DEFAULT_VERB, REG_SZ)?;
            Key::open(&target.command_key(), true)?.write_string(None, &command, REG_SZ)?;
        } else {
            restore_from_backup(&target)?;
        }
    }

    if enabled {
        // 标记必须在备份**之后**置上，否则首次安装会因为看到标记而跳过备份。
        Key::open(OWN_KEY, true)?.write_dword(Some(INSTALLED_MARKER), 1)?;
        tracing::info!("已将文件夹默认打开程序指向本程序");
    } else {
        tracing::info!("已将文件夹默认打开程序交还资源管理器");
    }

    // 不广播的话，已经在运行的资源管理器会继续用缓存里的旧处理程序，
    // 表现是「注册表明明改对了，双击文件夹却仍打开资源管理器」，要重启 explorer 才生效。
    notify_shell();
    Ok(())
}

/// 还原全部改动（右键菜单 + 默认打开程序）。
///
/// 卸载时应当调用它，否则文件夹会指向一个已经不存在的程序。
pub fn restore_all() -> anyhow::Result<()> {
    // 逐项尽力还原、收集失败而不是遇到第一个错误就返回：
    // 还原是「足控」场景下的抢救动作，中断在中途会把系统留在半接管状态 ——
    // 比「有一项没还原成功」更糟。这里把所有能做的都做完，最后统一报告。
    let mut failures = Vec::new();
    let mut attempt = |what: &str, result: anyhow::Result<()>| {
        if let Err(error) = result {
            failures.push(format!("{what}：{error}"));
        }
    };

    for target in HANDLER_TARGETS {
        attempt(target.progid, restore_from_backup(&target));
    }
    for (verb, ..) in CONTEXT_MENU_KEYS {
        attempt(verb, delete_tree(verb));
    }
    // 登记处最后删：前面的还原依赖其中的备份值。
    attempt(OWN_KEY, delete_tree(OWN_KEY));

    notify_shell();

    if failures.is_empty() {
        tracing::info!("文件夹接管已全部还原");
        return Ok(());
    }
    anyhow::bail!("部分还原失败：{}", failures.join("；"))
}

/// 一次性安装「右键菜单 + 默认打开程序」，并写下还原脚本。
pub fn install_all(exe_path: &Path) -> anyhow::Result<()> {
    set_context_menu(exe_path, true)?;
    set_default_handler(exe_path, true)?;
    // 还原脚本写失败不该让安装整体失败：注册表已经改好，
    // 脚本只是额外的一层保险。
    if let Err(error) = write_restore_script(exe_path) {
        tracing::warn!(%error, "还原脚本写入失败；仍可用「恢复资源管理器为默认」菜单项还原");
    }
    Ok(())
}

/// 右键作用于文件夹本身时，shell 传给我们的位置参数。
const SELECTED_ARGUMENT: &str = "%1";
/// 右键作用于文件夹内空白处时，shell 传给我们的是当前目录。
const BACKGROUND_ARGUMENT: &str = "%V";

/// 构造 shell 命令字符串。
///
/// 路径必须加引号：`C:\Program Files\...` 这类含空格的路径不加引号会被 shell
/// 拆成多个参数，于是它去找一个叫 `C:\Program` 的程序。
fn shell_command(exe_path: &Path, argument: &str) -> String {
    format!("\"{}\" \"{argument}\"", exe_path.display())
}

/// 读取某个键的默认值，判断是否就是我们要写入的那条命令。
///
/// 路径比较忽略大小写：Windows 路径本身不区分大小写，
/// 而注册表里的值可能被其他工具改写过大小写。
fn is_our_command(path: &str, expected: &str) -> bool {
    Key::open(path, false)
        .ok()
        .and_then(|key| key.read_string(None))
        .is_some_and(|(_, value)| value.eq_ignore_ascii_case(expected))
}

/// 覆盖前把原值备份下来。
///
/// 只有三种情况不备份：键不存在（HKLM 提供原始值，删掉我们的键即可恢复）、
/// 原值就是我们写的（重复安装）、以及已经备份过（不覆盖真实备份）。
fn back_up_existing(target: &HandlerTarget) -> anyhow::Result<()> {
    let own = Key::open(OWN_KEY, true)?;

    // 已经装过：当前值必然是我们写的，备份它会让还原把我们的命令又写回去。
    if own.read_dword(Some(INSTALLED_MARKER)).is_some() {
        return Ok(());
    }

    // 默认动词与命令各备份一份。
    // 工厂状态下 `...\shell` 的默认值本来就不存在，读不到是正常的 ——
    // 还原时「没有备份」会被翻译成「删掉我们写的值」，正好回到未设置。
    capture(
        &own,
        &target.shell_key(),
        &target.backup_verb(),
        &target.backup_verb_type(),
    )?;
    capture(
        &own,
        &target.command_key(),
        &target.backup_open(),
        &target.backup_open_type(),
    )
}

/// 把 `key_path` 的默认值抄进登记处的备份值里（含值类型）。
fn capture(own: &Key, key_path: &str, backup: &str, backup_type: &str) -> anyhow::Result<()> {
    // 已有备份就不再覆盖，否则重复安装会把真实原值冲掉。
    if own.read_string(Some(backup)).is_some() {
        return Ok(());
    }
    let Ok(source) = Key::open(key_path, false) else {
        // 键不存在是常态：默认值在 HKLM，删掉我们写的键就能恢复，无需备份。
        return Ok(());
    };
    let Some((kind, value)) = source.read_string(None) else {
        return Ok(());
    };

    own.write_string(Some(backup), &value, REG_SZ)?;
    own.write_dword(Some(backup_type), kind.0)?;
    tracing::info!("检测到用户自定义的设置，已备份原值：{key_path}");
    Ok(())
}

/// 还原一个 progid：有备份就回写，没有就删掉我们写的东西。
fn restore_from_backup(target: &HandlerTarget) -> anyhow::Result<()> {
    let own = Key::open(OWN_KEY, false).ok();

    restore_value(
        own.as_ref(),
        &target.shell_key(),
        &target.backup_verb(),
        &target.backup_verb_type(),
        // 默认动词是我们新建的**值**，没有备份就删掉值即可；
        // 整键删掉会波及用户可能已有的其它自定义动词。
        RestoreMode::Value,
    )?;
    restore_value(
        own.as_ref(),
        &target.command_key(),
        &target.backup_open(),
        &target.backup_open_type(),
        // 命令子键整棵都是我们建的，没有备份就整棵删掉最干净。
        RestoreMode::Tree,
    )
}

/// 没有备份时如何撤销我们写的东西。
enum RestoreMode {
    /// 只删这个值，保留键。
    Value,
    /// 整棵键树删掉。
    Tree,
}

fn restore_value(
    own: Option<&Key>,
    key_path: &str,
    backup: &str,
    backup_type: &str,
    mode: RestoreMode,
) -> anyhow::Result<()> {
    if let Some(own) = own {
        if let Some((_, previous)) = own.read_string(Some(backup)) {
            // 类型必须照原样写回：把 `REG_EXPAND_SZ` 还原成 `REG_SZ`
            // 会让原值里的 `%SystemRoot%` 不再展开。
            let kind = own.read_dword(Some(backup_type)).map_or(REG_SZ, REG_VALUE_TYPE);
            return Key::open(key_path, true)?.write_string(None, &previous, kind);
        }
    }

    match mode {
        RestoreMode::Value => {
            // 键不存在说明我们本来就没写过它，无需处理。
            // 注意必须用读写权限打开：只读句柄上删值会被拒绝。
            if let Ok(key) = Key::open_for_write(key_path) {
                key.delete_value(None)?;
            }
            Ok(())
        }
        RestoreMode::Tree => delete_tree(key_path),
    }
}

/// 读取某个 `...\shell` 键的默认值，也就是该 progid 的默认动词名。
fn reads_verb(shell_key: &str) -> Option<String> {
    Key::open(shell_key, false)
        .ok()
        .and_then(|key| key.read_string(None))
        .map(|(_, value)| value)
}

/// 广播「关联已变更」，让 shell 立即丢弃缓存的旧处理程序。
///
/// 不广播的话，已经在运行的资源管理器不会知道关联变了，
/// 必须重启 `explorer.exe` 才生效 —— 用户看到的就是「改了但没用」。
fn notify_shell() {
    unsafe {
        SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None);
    }
}

/// 删除一棵键树。
///
/// 路径相对于 `HKEY_CURRENT_USER`，因此不带 `HKCU\` 前缀。
/// 键不存在时返回「找不到」，对删除而言就是成功 —— 卸载时键可能已被外部工具删掉。
fn delete_tree(path: &str) -> anyhow::Result<()> {
    let wide = to_wide(path);
    let status = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(wide.as_ptr())) };
    if status == ERROR_SUCCESS || status == ERROR_FILE_NOT_FOUND || status == ERROR_PATH_NOT_FOUND {
        return Ok(());
    }
    anyhow::bail!("删除注册表键失败 {path}（错误码 {}）", status.0);
}

/// 转成以 NUL 结尾的 UTF-16。
fn to_wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 键句柄的 RAII 包装。
///
/// 用它而不是裸句柄，是因为本模块有多条提前返回的错误路径，
/// 靠人工在每个分支记得 `RegCloseKey` 一定会漏。
struct Key(HKEY);

impl Key {
    /// 打开或创建键（读写权限）。`create` 为假时，键不存在返回错误。
    fn open(path: &str, create: bool) -> anyhow::Result<Self> {
        let wide = to_wide(path);
        let mut raw = HKEY(std::ptr::null_mut());
        let status = unsafe {
            if create {
                RegCreateKeyExW(
                    HKEY_CURRENT_USER,
                    PCWSTR(wide.as_ptr()),
                    None,
                    PCWSTR::null(),
                    REG_OPTION_NON_VOLATILE,
                    KEY_READ | KEY_WRITE,
                    None,
                    &mut raw,
                    None,
                )
            } else {
                RegOpenKeyExW(
                    HKEY_CURRENT_USER,
                    PCWSTR(wide.as_ptr()),
                    None,
                    KEY_READ,
                    &mut raw,
                )
            }
        };

        if status != ERROR_SUCCESS {
            anyhow::bail!("打开注册表键失败 {path}（错误码 {}）", status.0);
        }
        Ok(Self(raw))
    }

    /// 以**读写**权限打开一个已存在的键，不创建。
    ///
    /// 需要它是因为 [`Self::open`] 在 `create = false` 时只请求 `KEY_READ`：
    /// 拿这样的句柄去删值会被拒绝（错误码 5），而调用点看起来完全正常。
    fn open_for_write(path: &str) -> anyhow::Result<Self> {
        let wide = to_wide(path);
        let mut raw = HKEY(std::ptr::null_mut());
        let status = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(wide.as_ptr()),
                None,
                KEY_READ | KEY_WRITE,
                &mut raw,
            )
        };

        if status != ERROR_SUCCESS {
            anyhow::bail!("打开注册表键（读写）失败 {path}（错误码 {}）", status.0);
        }
        Ok(Self(raw))
    }

    /// 读取字符串值，返回（值的类型，内容）。`name` 为 `None` 表示默认值。
    fn read_string(&self, name: Option<&str>) -> Option<(REG_VALUE_TYPE, String)> {
        let name_wide = name.map(to_wide);
        let name_pointer = name_wide
            .as_ref()
            .map_or(PCWSTR::null(), |wide| PCWSTR(wide.as_ptr()));

        // 第一趟只问长度：值有多大，事先不知道。
        let mut size = 0u32;
        let status =
            unsafe { RegQueryValueExW(self.0, name_pointer, None, None, None, Some(&mut size)) };
        if status != ERROR_SUCCESS || size == 0 {
            return None;
        }

        let mut buffer = vec![0u8; size as usize];
        let mut kind = REG_VALUE_TYPE(0);
        let status = unsafe {
            RegQueryValueExW(
                self.0,
                name_pointer,
                None,
                Some(&mut kind),
                Some(buffer.as_mut_ptr()),
                Some(&mut size),
            )
        };
        if status != ERROR_SUCCESS {
            return None;
        }

        // 第二趟返回的 `size` 是实际写入的字节数，可能小于第一趟的问询值，
        // 因此切片长度要用第二趟的 `size` 而不是 `buffer.len()`。
        let units = (size as usize) / std::mem::size_of::<u16>();
        let wide = unsafe { std::slice::from_raw_parts(buffer.as_ptr() as *const u16, units) };
        // 结尾的 NUL 是注册表字符串的组成部分，展示给调用方时要剥掉。
        Some((
            kind,
            String::from_utf16_lossy(wide)
                .trim_end_matches('\0')
                .to_owned(),
        ))
    }

    /// 写入字符串值。`name` 为 `None` 表示写默认值。
    fn write_string(&self, name: Option<&str>, value: &str, kind: REG_VALUE_TYPE) -> anyhow::Result<()> {
        // 注册表里的 `REG_SZ` / `REG_EXPAND_SZ` 都是 UTF-16 且**必须**以 NUL 结尾，
        // 长度含结尾的 NUL。漏掉结尾会让下一个读到它的程序越界读。
        let wide = to_wide(value);
        let bytes = unsafe { std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2) };
        self.write_raw(name, bytes, kind)
    }

    /// 写入 DWORD 值。
    fn write_dword(&self, name: Option<&str>, value: u32) -> anyhow::Result<()> {
        self.write_raw(name, &value.to_ne_bytes(), REG_DWORD)
    }

    /// 读取 DWORD 值。值不存在或大小不是 4 字节时返回 `None`。
    fn read_dword(&self, name: Option<&str>) -> Option<u32> {
        let mut buffer = [0u8; 4];
        let mut size = buffer.len() as u32;
        let status = unsafe {
            self.query(name, None, Some(buffer.as_mut_ptr()), Some(&mut size))
        };
        // 大小不符说明它不是 DWORD（例如被写成了字符串），宁可当作没有备份。
        if status != ERROR_SUCCESS || size != buffer.len() as u32 {
            return None;
        }
        Some(u32::from_ne_bytes(buffer))
    }

    fn write_raw(&self, name: Option<&str>, bytes: &[u8], kind: REG_VALUE_TYPE) -> anyhow::Result<()> {
        let name_wide = name.map(to_wide);
        let name_pointer = name_wide
            .as_ref()
            .map_or(PCWSTR::null(), |wide| PCWSTR(wide.as_ptr()));

        let status = unsafe { RegSetValueExW(self.0, name_pointer, None, kind, Some(bytes)) };
        if status != ERROR_SUCCESS {
            anyhow::bail!("写入注册表值失败（错误码 {}）", status.0);
        }
        Ok(())
    }

    /// 删除一个值。`name` 为 `None` 表示删除默认值。
    fn delete_value(&self, name: Option<&str>) -> anyhow::Result<()> {
        let name_wide = name.map(to_wide);
        let name_pointer = name_wide
            .as_ref()
            .map_or(PCWSTR::null(), |wide| PCWSTR(wide.as_ptr()));

        let status = unsafe { RegDeleteValueW(self.0, name_pointer) };
        // 值不存在视为成功：还原时它可能已被外部工具删掉。
        if status == ERROR_SUCCESS || status == ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        anyhow::bail!("删除注册表值失败（错误码 {}）", status.0);
    }

    /// `RegQueryValueExW` 的薄封装：只负责把 `Option<&str>` 值名转成指针，
    /// 省掉每个调用点重复写三行转换。
    unsafe fn query(
        &self,
        name: Option<&str>,
        kind: Option<&mut REG_VALUE_TYPE>,
        data: Option<*mut u8>,
        size: Option<&mut u32>,
    ) -> windows::Win32::Foundation::WIN32_ERROR {
        let name_wide = name.map(to_wide);
        let name_pointer = name_wide
            .as_ref()
            .map_or(PCWSTR::null(), |wide| PCWSTR(wide.as_ptr()));
        unsafe {
            RegQueryValueExW(
                self.0,
                name_pointer,
                None,
                kind.map(std::ptr::from_mut),
                data,
                size.map(std::ptr::from_mut),
            )
        }
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

/// 在程序目录旁边写一个还原脚本。
///
/// 这是本功能唯一的「程序坏掉后还能自救」通道：覆盖生效后若程序被删除或移动，
/// 双击文件夹会打不开，而那时程序本身已经跑不起来。
///
/// 脚本内容刻意保持纯 ASCII：`.bat` 在中文系统上按 GBK 解释，
/// 写 UTF-8 中文会显示成乱码。
fn write_restore_script(exe_path: &Path) -> anyhow::Result<()> {
    let Some(directory) = exe_path.parent() else {
        return Ok(());
    };

    let mut script = String::from("@echo off\r\n");
    script.push_str("rem Restore Windows Explorer as the default folder handler.\r\n");
    script.push_str("rem Generated by Files. Safe to run at any time.\r\n");

    for target in HANDLER_TARGETS {
        // `\shell\open` 整棵删掉，双击就不会再走我们的命令。
        script.push_str(&format!(
            "reg delete \"HKCU\\{}\\shell\\open\" /f >nul 2>&1\r\n",
            target.progid
        ));
        // 默认动词也要清掉，否则会留下一个指向已消失的 `open` 动词的默认值。
        // 置成空串等价于工厂状态下的「未设置」。
        script.push_str(&format!(
            "reg add \"HKCU\\{}\\shell\" /ve /d \"\" /f >nul 2>&1\r\n",
            target.progid
        ));
    }
    for (verb, ..) in CONTEXT_MENU_KEYS {
        script.push_str(&format!("reg delete \"HKCU\\{verb}\" /f >nul 2>&1\r\n"));
    }
    script.push_str(&format!("reg delete \"HKCU\\{OWN_KEY}\" /f >nul 2>&1\r\n"));

    // 批处理里没有 `SHChangeNotify` 可调，只能重启资源管理器让它重新读取关联。
    // 灾难恢复场景下这是可以接受的代价：不重启的话改了也不生效。
    script.push_str("taskkill /f /im explorer.exe >nul 2>&1\r\n");
    script.push_str("start explorer.exe\r\n");
    script.push_str("echo Explorer restored as the default folder handler.\r\n");
    script.push_str("pause\r\n");

    std::fs::write(directory.join(RESTORE_SCRIPT), script)
        .with_context(|| format!("写入还原脚本失败（目录 {}）", directory.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    // 只在测试里用到：验证「值类型必须原样保留」这条性质。
    use windows::Win32::System::Registry::REG_EXPAND_SZ;

    #[test]
    fn shell_command_quotes_both_path_and_argument() {
        // 含空格的路径不加引号会被 shell 拆成多个参数，
        // 于是它去找一个叫 `C:\Program` 的程序。
        let command = shell_command(Path::new(r"C:\Program Files\Files\files-gpui.exe"), "%1");
        assert_eq!(command, r#""C:\Program Files\Files\files-gpui.exe" "%1""#);
    }

    #[test]
    fn shell_command_uses_the_given_argument_verbatim() {
        // 右键背景用 `%V`（当前目录），右键文件夹本身用 `%1`（选中项）。
        assert!(shell_command(Path::new(r"D:\app.exe"), "%V").ends_with(r#""%V""#));
    }

    #[test]
    fn handler_targets_cover_every_entry_point() {
        // 少一个就有一类入口漏掉：文件夹、盘符，以及作为它们公共父级的 Folder。
        let progids: Vec<&str> = HANDLER_TARGETS.iter().map(|target| target.progid).collect();
        for expected in [
            r"Software\Classes\Folder",
            r"Software\Classes\Directory",
            r"Software\Classes\Drive",
        ] {
            assert!(progids.contains(&expected), "缺少 progid {expected}");
        }
    }

    #[test]
    fn handler_keys_are_derived_from_the_progid() {
        for target in HANDLER_TARGETS {
            assert_eq!(target.shell_key(), format!(r"{}\shell", target.progid));
            assert_eq!(
                target.command_key(),
                format!(r"{}\shell\open\command", target.progid)
            );
            // 默认动词写在 `\shell` 上，命令写在 `\shell\open\command` 上：
            // 写成同一个键就会出现「写对了命令但双击不走」。
            assert_ne!(target.shell_key(), target.command_key());
        }
    }

    #[test]
    fn backup_names_are_unique_across_targets() {
        // 备份名撞车会让三个 progid 互相覆盖备份，还原时写出错误的值 ——
        // 而且不会报错，只是行为诡异。
        let mut names: Vec<String> = Vec::new();
        for target in HANDLER_TARGETS {
            names.push(target.backup_verb());
            names.push(target.backup_verb_type());
            names.push(target.backup_open());
            names.push(target.backup_open_type());
        }
        let total = names.len();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), total, "备份值名存在重复");
    }

    #[test]
    fn context_menu_and_handler_keys_do_not_overlap() {
        // 两类改动落在不同的键上；如果重叠，卸载其中一项会连带破坏另一项。
        let handlers: Vec<String> = HANDLER_TARGETS
            .iter()
            .flat_map(|target| [target.shell_key(), target.command_key()])
            .collect();
        for (verb, command, _) in CONTEXT_MENU_KEYS {
            assert!(!handlers.iter().any(|key| key == verb), "{verb} 与处理器键重叠");
            assert!(!handlers.iter().any(|key| key == command), "{command} 与处理器键重叠");
        }
    }

    #[test]
    fn notify_shell_is_callable() {
        // 广播本身没有返回值也没有副作用可断言，但至少要保证调用路径是通的
        // （签名或 feature 配错时这里会直接编译不过）。
        notify_shell();
    }

    #[test]
    fn command_keys_are_children_of_their_verb_keys() {
        // 命令必须写在动词键的 `\command` 子键里，而不是动词键自己的默认值 ——
        // 后者是菜单显示文字。之前就是在这里读错，导致「注册表明明写了、
        // 状态却报未安装」，进而每次进菜单都重复安装一遍。
        for (verb, command, argument) in CONTEXT_MENU_KEYS {
            assert_eq!(command, format!(r"{verb}\command"), "动词键 {verb} 的命令子键不对");
            assert!(
                argument == "%1" || argument == "%V",
                "位置参数只能是 %1 或 %V，得到 {argument}"
            );
        }
        // 两处挂载点的位置参数必须不同：文件夹本身有选中项（%1），
        // 文件夹空白处没有（%V）。用错会传入空串，程序会退回「此电脑」。
        assert_ne!(CONTEXT_MENU_KEYS[0].2, CONTEXT_MENU_KEYS[1].2);
    }

    #[test]
    fn wide_conversion_appends_exactly_one_nul() {
        assert_eq!(to_wide("a"), vec![b'a' as u16, 0]);
        assert_eq!(to_wide(""), vec![0]);
        assert_eq!(to_wide("文"), vec!['文' as u16, 0]);
    }

    /// 注册表读写回环。只在我们自己的子树里做，不碰系统关联。
    #[test]
    fn registry_round_trip_preserves_text_and_type() {
        const TEST_KEY: &str = r"Software\Files\__test_round_trip";

        let key = Key::open(TEST_KEY, true).expect("创建测试键");
        key.write_string(Some("Ascii"), r"C:\Windows", REG_SZ)
            .expect("写 ASCII");
        // 中文路径是最容易在 UTF-16 长度换算上出错的情形。
        key.write_string(Some("Chinese"), r"D:\我的文档\项目", REG_EXPAND_SZ)
            .expect("写中文");
        key.write_dword(Some("Kind"), 2).expect("写 DWORD");

        let key = Key::open(TEST_KEY, false).expect("重新打开测试键");
        let (kind, ascii) = key.read_string(Some("Ascii")).expect("读 ASCII");
        assert_eq!(ascii, r"C:\Windows");
        assert_eq!(kind, REG_SZ);

        let (kind, chinese) = key.read_string(Some("Chinese")).expect("读中文");
        assert_eq!(chinese, r"D:\我的文档\项目");
        // 类型必须原样保留：把 `REG_EXPAND_SZ` 写回成 `REG_SZ`
        // 会让原值里的 `%VAR%` 不再展开。
        assert_eq!(kind, REG_EXPAND_SZ);

        assert_eq!(key.read_dword(Some("Kind")), Some(2));
        // 不存在的值返回 `None`，而不是报错或空串。
        assert!(key.read_string(Some("Missing")).is_none());
        assert!(key.read_dword(Some("Missing")).is_none());
        // 类型不符（这里是字符串）也要当作没有，避免还原时写回垃圾。
        assert!(key.read_dword(Some("Ascii")).is_none());

        delete_tree(TEST_KEY).expect("清理测试键");
    }

    #[test]
    fn deleting_a_missing_key_is_not_an_error() {
        // 卸载时键可能已被外部工具删掉，此时必须静默成功。
        delete_tree(r"Software\Files\__test_never_existed").expect("删除不存在的键应成功");
    }

    #[test]
    fn a_missing_key_is_never_mistaken_for_ours() {
        assert!(!is_our_command(
            r"Software\Files\__test_never_existed",
            r#""D:\app.exe" "%1""#
        ));
    }

    #[test]
    fn installed_command_is_recognised_and_foreign_ones_are_not() {
        const TEST_KEY: &str = r"Software\Files\__test_state";
        let exe = Path::new(r"C:\Program Files\Files\files-gpui.exe");
        let command = shell_command(exe, SELECTED_ARGUMENT);

        Key::open(TEST_KEY, true)
            .expect("创建测试键")
            .write_string(None, &command, REG_SZ)
            .expect("写命令");

        assert!(is_our_command(TEST_KEY, &command));
        // 大小写不同也要认出来：Windows 路径不区分大小写。
        assert!(is_our_command(
            TEST_KEY,
            &shell_command(Path::new(r"c:\program files\files\FILES-GPUI.EXE"), SELECTED_ARGUMENT)
        ));
        // 换一个程序就应当不匹配，否则会误判成「已安装」而不去安装。
        assert!(!is_our_command(
            TEST_KEY,
            &shell_command(Path::new(r"D:\other.exe"), SELECTED_ARGUMENT)
        ));
        // 同一个程序但参数不同也必须不匹配：
        // 否则背景菜单项的 `%V` 会被误认成文件夹项的 `%1`。
        assert!(!is_our_command(TEST_KEY, &shell_command(exe, BACKGROUND_ARGUMENT)));

        delete_tree(TEST_KEY).expect("清理测试键");
    }
}
