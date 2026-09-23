//! Windows Shell 右键菜单（本次仅预留架构位）。
//!
//! 规划实现：走 `IContextMenu` / `IShellFolder` 拿到系统原生的动词列表，
//! 需要在主线程（STA 公寓）上通过 `TrackPopupMenuEx` 弹出。
//!
//! 备选简化路线：若原生菜单的 COM 接线成本过高，MVP 阶段可先自绘固定项
//! （打开、复制、粘贴、重命名、删除、属性），trait 边界保持不变。
