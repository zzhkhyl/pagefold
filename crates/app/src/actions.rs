//! GPUI Action 定义与键位绑定。
//!
//! 依据 PLAN.md「交互与动效」章节，需要注册的键位为：
//! - `Ctrl+T` 新建标签、`Ctrl+W` 关闭标签、`Ctrl+L` 聚焦地址栏
//! - `Alt+Left` / `Alt+Right` 前进后退
//! - `Ctrl+1` / `Ctrl+2` 切换视图模式、`Ctrl+3` 切换主题
//!
//! 骨架阶段尚无标签条与视图可绑定，因此本模块暂为空壳；
//! 待 `fm-shell` 与 `fm-views` 提供真实视图后，在此用 `gpui::actions!` 声明 Action
//! 并在 `bootstrap::run` 中通过 `cx.bind_keys` 注册。
