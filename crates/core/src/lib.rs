//! 领域模型层。
//!
//! 依赖约束（由分层架构决定）：本 crate **不得**依赖 gpui、任何 Windows API 或
//! 文件系统实现。它只描述「数据长什么样」与「标签页状态如何流转」，
//! 从而让 `platform` / `services` 能在任意宿主上做静态检查。

pub mod command;
pub mod entry;
pub mod location;
pub mod selection;
pub mod tab;
pub mod tabs;
pub mod util;

pub use command::Command;
pub use entry::{Entry, EntryKind, ExtId, IconId, SortDirection, SortKey, ViewMode};
pub use location::Location;
pub use selection::Selection;
pub use tab::TabState;
pub use tabs::{Tab, TabId, TabsState};
pub use util::format_size;
