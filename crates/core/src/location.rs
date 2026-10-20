//! 标签页位置：真实目录或虚拟位置。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 标签页所处的位置。
///
/// 「此电脑」不是文件系统目录，无法用 `PathBuf` 表示。
/// 早先把位置直接存成 `PathBuf`，这类虚拟位置就无处安放 ——
/// 若改用哨兵字符串冒充路径，它会被一路喂给 `FileSystem::scan`，
/// 用户看到的是「路径不存在」这种莫名其妙的报错。
///
/// 用枚举之后，「哪里需要真实目录」由类型系统强制标注：
/// 每个使用点都要显式处理两种情形，漏掉处理会直接编译不过。
///
/// ## 为什么设置页也是「位置」
///
/// 设置页做成**标签内页面**而不是独立窗口：这样它天然进入 [`crate::TabState`]
/// 的历史栈、标签条、`Ctrl+W` 与「重新打开关闭的标签页」体系，
/// 不需要另一套窗口生命周期、主题同步与焦点管理。
/// `as_dir()` 对它返回 `None`，因此类型系统保证它**永远不会**被送去扫描目录。
///
/// 序列化形态（会话持久化用）是 serde 的默认外部标记：
/// `"Computer"` / `{"Directory":"C:\\Users"}`。刻意不做自定义 tag —
/// 默认形态对 unit 变体最紧凑，且不需要为「新变体」维护一份 tag 映射表。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Location {
    /// 虚拟位置：本机（驱动器总览）。
    Computer,
    /// 虚拟位置：回收站。
    ///
    /// **只读浏览**：列出被删除的项目（原路径、大小、删除时间），
    /// 不提供还原与清空——那两条是写操作，属后续阶段。
    RecycleBin,
    /// 虚拟位置：设置页（同一窗口内的一个标签）。
    Settings,
    /// 真实文件系统目录。
    Directory(PathBuf),
}

impl Location {
    /// 真实目录路径；虚拟位置返回 `None`。
    ///
    /// 所有需要把位置交给文件系统的地方都必须先过这一关，
    /// 从而不可能把虚拟位置误当成目录去扫描。
    pub fn as_dir(&self) -> Option<&Path> {
        match self {
            Self::Directory(path) => Some(path),
            Self::Computer | Self::RecycleBin | Self::Settings => None,
        }
    }

    /// 是否是一个不承载文件条目的虚拟位置（「此电脑」/ 回收站 / 设置）。
    ///
    /// 供「内容区该画什么」这类分派使用：这三者的 `entries` 恒为空，
    /// 若按「空文件夹」处理就会画出错误的空状态。
    pub fn is_virtual(&self) -> bool {
        !matches!(self, Self::Directory(_))
    }

    /// 标签页标题。
    pub fn title(&self) -> String {
        match self {
            // 虚拟位置没有文件名，直接用固定文案。
            Self::Computer => String::from("此电脑"),
            Self::RecycleBin => String::from("回收站"),
            Self::Settings => String::from("设置"),
            // 根目录（如 `C:\`）没有最后一段，退回完整路径，
            // 否则标签标题会变成空串，用户看到的是一个没有文字的标签。
            Self::Directory(path) => path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string()),
        }
    }

    /// 是否正指向目录 `dir`。用于侧边栏选中态比较。
    pub fn is_dir(&self, dir: &Path) -> bool {
        matches!(self, Self::Directory(path) if path == dir)
    }

    /// 从「由 shell 交给我们的位置参数」解析位置。
    ///
    /// 这个入口专门用于命令行调用，与内部状态转换的关键差异是它**必须处理虚拟项的
    /// CLSID 路径**：桌面上的「此电脑」「回收站」「网络」都是 shell 命名空间里的虚拟项，
    /// 当它们作为「文件夹」被交给外部程序时，命令行里拿到的是 `::{GUID}` 形式的字符串，
    /// 而不是文件系统路径。直接当成目录去扫描，用户看到的是一个空列表加一条报错。
    pub fn from_shell_argument(argument: &str) -> Self {
        // 「此电脑」的 CLSID。桌面上双击「此电脑」图标时传过来的就是它。
        const COMPUTER_CLSID: &str = "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}";
        // 「回收站」的 CLSID。桌面上的回收站图标被双击时传过来的是它。
        const RECYCLE_BIN_CLSID: &str = "::{645FF040-5081-101B-9F08-00AA002F954E}";

        let trimmed = argument.trim();
        if trimmed.is_empty() {
            return Self::Computer;
        }
        if trimmed.eq_ignore_ascii_case("computer") || trimmed == "此电脑" {
            return Self::Computer;
        }
        if trimmed.eq_ignore_ascii_case(RECYCLE_BIN_CLSID)
            || trimmed.eq_ignore_ascii_case("recyclebin")
            || trimmed == "回收站"
        {
            return Self::RecycleBin;
        }
        // 设置页也接受命令行关键字：`pagefold.exe settings` 可以直接落到设置，
        // 与「此电脑」「回收站」保持同一套入口规则。
        if trimmed.eq_ignore_ascii_case("settings")
            || trimmed.eq_ignore_ascii_case("preferences")
            || trimmed == "设置"
        {
            return Self::Settings;
        }
        if trimmed.eq_ignore_ascii_case(COMPUTER_CLSID) {
            return Self::Computer;
        }
        // 其余 `::{...}`（网络、控制面板…）与 `shell:` 伪协议仍是我们无法表示的虚拟位置。
        // 退回「此电脑」而不是报错：用户点桌面图标时期待的是「开一个文件管理器」，
        // 给一个能用的起点比弹一条错误更有意义。
        if trimmed.starts_with("::{") || trimmed.starts_with("shell:") {
            return Self::Computer;
        }
        Self::Directory(PathBuf::from(trimmed))
    }
}

impl From<PathBuf> for Location {
    fn from(path: PathBuf) -> Self {
        Self::Directory(path)
    }
}

impl From<&Path> for Location {
    fn from(path: &Path) -> Self {
        Self::Directory(path.to_path_buf())
    }
}

impl From<&PathBuf> for Location {
    fn from(path: &PathBuf) -> Self {
        Self::Directory(path.clone())
    }
}

/// 把字符串按**目录路径**解释。
///
/// 提供它是为了让 `Location` 保持 `PathBuf` 那样的书写便利
/// （`TabState::new(r"C:\Users")`），否则每个调用点都要写一遍 `PathBuf::from`。
/// 需要表达虚拟位置时必须显式写 `Location::Computer`，不会因为传了字符串而误入。
impl From<&str> for Location {
    fn from(path: &str) -> Self {
        Self::Directory(PathBuf::from(path))
    }
}

impl From<String> for Location {
    fn from(path: String) -> Self {
        Self::Directory(PathBuf::from(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computer_has_no_directory() {
        assert_eq!(Location::Computer.as_dir(), None);
    }

    #[test]
    fn directory_exposes_its_path() {
        let location = Location::from(PathBuf::from(r"C:\Users"));
        assert_eq!(location.as_dir(), Some(Path::new(r"C:\Users")));
    }

    #[test]
    fn titles_are_derived_from_the_location() {
        assert_eq!(Location::Computer.title(), "此电脑");
        assert_eq!(Location::from(PathBuf::from(r"C:\Users\abc")).title(), "abc");
        // 根目录没有最后一段，必须退回完整路径而不是空串。
        assert_eq!(Location::from(PathBuf::from(r"C:\")).title(), r"C:\");
    }

    #[test]
    fn is_dir_only_matches_real_directories() {
        let location = Location::from(PathBuf::from(r"C:\Users"));
        assert!(location.is_dir(Path::new(r"C:\Users")));
        assert!(!location.is_dir(Path::new(r"C:\Windows")));
        // 虚拟位置永远不匹配任何目录，否则侧边栏会误高亮。
        assert!(!Location::Computer.is_dir(Path::new(r"C:\Users")));
    }

    #[test]
    fn conversions_from_paths_are_lossless() {
        let owned = PathBuf::from(r"D:\a");
        assert_eq!(Location::from(owned.clone()).as_dir(), Some(owned.as_path()));
        assert_eq!(
            Location::from(owned.as_path()).as_dir(),
            Some(owned.as_path())
        );
        assert_eq!(Location::from(&owned).as_dir(), Some(owned.as_path()));
        // 字符串一律按目录解释，保证 `Location` 的书写便利度不低于 `PathBuf`。
        assert_eq!(Location::from(r"D:\a").as_dir(), Some(owned.as_path()));
        assert_eq!(
            Location::from(String::from(r"D:\a")).as_dir(),
            Some(owned.as_path())
        );
    }

    #[test]
    fn shell_argument_accepts_plain_paths() {
        assert_eq!(
            Location::from_shell_argument(r"C:\Users\abc"),
            Location::from(PathBuf::from(r"C:\Users\abc"))
        );
        // 驱动器根目录也必须当成真实目录，而不是被路径形态判断误判成虚拟项。
        assert_eq!(
            Location::from_shell_argument(r"D:\"),
            Location::from(PathBuf::from(r"D:\"))
        );
    }

    #[test]
    fn settings_is_a_virtual_location_with_its_own_title() {
        // 设置页是「位置」而不是目录：它必须能被标签页承载，但不能被送去扫描。
        assert_eq!(Location::Settings.as_dir(), None);
        assert_eq!(Location::Settings.title(), "设置");
        assert!(Location::Settings.is_virtual());
        assert!(!Location::Settings.is_dir(Path::new(r"C:\Users")));
        // 与「此电脑」同属虚拟位置，但两者在标签标题上必须可区分。
        assert_ne!(Location::Settings.title(), Location::Computer.title());
    }

    #[test]
    fn shell_argument_recognises_the_settings_page() {
        for argument in ["settings", "SETTINGS", "preferences", "设置"] {
            assert_eq!(
                Location::from_shell_argument(argument),
                Location::Settings,
                "参数 {argument:?} 应映射到设置页"
            );
        }
    }

    #[test]
    fn location_round_trips_through_json() {
        // 会话持久化依赖这条往返：标签的历史里三种位置都可能出现。
        for location in [
            Location::Computer,
            Location::RecycleBin,
            Location::Settings,
            Location::Directory(PathBuf::from(r"C:\Users\abc")),
        ] {
            let text = serde_json::to_string(&location).expect("位置应可序列化");
            let parsed: Location = serde_json::from_str(&text).expect("位置应可反序列化");
            assert_eq!(parsed, location, "{text}");
        }
    }

    #[test]
    fn shell_argument_maps_computer_spellings_to_the_virtual_location() {
        for argument in [
            "",
            "   ",
            "computer",
            "COMPUTER",
            "此电脑",
            "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}",
            "::{20d04fe0-3aea-1069-a2d8-08002b30309d}",
        ] {
            assert_eq!(
                Location::from_shell_argument(argument),
                Location::Computer,
                "参数 {argument:?} 应映射到「此电脑」"
            );
        }
    }

    #[test]
    fn shell_argument_recognises_the_recycle_bin() {
        // 回收站现在有自己的虚拟位置，不能再像以前那样退回「此电脑」。
        for argument in [
            "::{645FF040-5081-101B-9F08-00AA002F954E}",
            "::{645ff040-5081-101b-9f08-00aa002f954e}",
            "recyclebin",
            "RecycleBin",
            "回收站",
        ] {
            assert_eq!(
                Location::from_shell_argument(argument),
                Location::RecycleBin,
                "参数 {argument:?} 应映射到「回收站」"
            );
        }
    }

    #[test]
    fn shell_argument_falls_back_instead_of_failing_on_other_virtual_items() {
        // 这些是无法用 `PathBuf` 表示的虚拟项，退回「此电脑」而不是产出一个假路径 ——
        // 否则用户点桌面上的图标会看到空列表加一条「路径不存在」。
        // 注意「回收站」不在这一列：它有对应的 `Location` 变体，见上一个测试。
        for argument in [
            "::{F02C1A0D-BE21-4350-88B0-7367FC96EF3C}", // 网络
            "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}\\foo", // 虚拟项下的子路径
            "shell:Downloads",
            "shell:startup",
        ] {
            assert_eq!(
                Location::from_shell_argument(argument),
                Location::Computer,
                "参数 {argument:?} 应退回「此电脑」"
            );
        }
    }
}
