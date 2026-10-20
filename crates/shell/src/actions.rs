//! 键盘驱动的用户动作（GPUI `Action`）。
//!
//! 定义在这里而不是 `fm-app`：`cx.bind_keys` 在应用层（`bootstrap`），
//! 而 `on_action` 的处理器在 `fm-shell`，两者必须看到同一组类型——
//! `fm-app` 依赖 `fm-shell`，反过来则成环，所以动作只能落在 `fm-shell`。
//!
//! 键位表以 `design/file-manager.html` 的全局 keydown 为准（那是设计的权威清单），
//! 另加 `Ctrl+L`（聚焦地址栏）与 `Ctrl+1/2/3`（视图 / 主题）——它们来自原
//! `fm-app::actions` 的规划注释，属设计稿未写但资源管理器惯例的快捷键。

use gpui::actions;

actions!(
    file_manager,
    [
        NewTab,
        CloseTab,
        ReopenClosedTab,
        GoBack,
        GoForward,
        GoUp,
        Refresh,
        FocusAddressBar,
        SetViewDetails,
        SetViewIcons,
        ToggleTheme,
        SelectAll,
        CopyItems,
        CutItems,
        PasteItems,
        RenameItem,
        DeleteItems,
        OpenItem,
        MoveCursorUp,
        MoveCursorDown,
        ExtendCursorUp,
        ExtendCursorDown,
    ]
);
