//! 外壳层：窗口的骨架布局。
//!
//! 自上而下四个区块（尺寸与间距取自 PLAN.md「窗口布局」章节）：
//! 1. 顶部工具栏 —— 标签条（上）+ 命令条（下），约 88px；
//! 2. 主区域 —— 左侧导航栏（展开 240px / 折叠 48px）+ 主内容区；
//! 3. 底部状态栏 —— 28px。
//!
//! 本 crate 只负责「把内容摆在正确的位置」，不负责内容本身：
//! 面包屑、图标网格、详情表格由 `fm-views` 提供。
//!
//! 实现上，各区块的渲染方法以 `impl ShellView` 的形式分散在各个子模块里。
//! 这样做是为了让所有区块共享同一份状态而不必跨模块传参，
//! 同时保持「一个区块一个文件」的可读性。

pub mod actions;
pub mod context_menu;
pub mod layout;
pub mod shell;
pub mod sidebar;
pub mod status_bar;
pub mod tab_strip;
pub mod titlebar;
pub mod toolbar;

pub use shell::ShellView;
