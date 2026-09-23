//! UI 交互产生的命令。
//!
//! 数据流：UI 产生 `Command` → `fm-app` 层分发 → `fm-services` 在
//! `BackgroundExecutor` 上执行 → 分批结果回传并更新 `TabState` → 触发重渲染。
//! 把交互统一收敛成命令枚举，是为了让键位绑定、工具栏按钮与后续的右键菜单
//! 三种入口共用同一条执行路径。

use std::path::PathBuf;

use crate::entry::ViewMode;

/// 一次用户交互。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// 跳转到指定目录。
    NavigateTo(PathBuf),
    /// 后退一步。
    Back,
    /// 前进一步。
    Forward,
    /// 进入上级目录。
    Up,
    /// 重新扫描当前目录（不改变历史）。
    Refresh,

    /// 新建标签页。
    NewTab,
    /// 关闭指定标签页。
    CloseTab(usize),
    /// 切换到指定标签页。
    SelectTab(usize),

    /// 切换内容区视图模式。
    SetViewMode(ViewMode),
    /// 折叠/展开左侧导航栏。
    ToggleSidebar,
    /// 切换深色/浅色主题。
    ToggleTheme,
}
