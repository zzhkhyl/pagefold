//! 领域模型层。
//!
//! 依赖约束（由分层架构决定）：本 crate **不得**依赖 gpui、任何 Windows API 或
//! 文件系统实现。它只描述「数据长什么样」与「标签页状态如何流转」，
//! 从而让 `platform` / `services` 能在任意宿主上做静态检查。

pub mod appearance;
pub mod command;
pub mod entry;
pub mod icon;
pub mod location;
pub mod naming;
pub mod selection;
pub mod shortcut;
pub mod tab;
pub mod tabs;
pub mod undo;
pub mod util;

pub use appearance::{Column, ColumnWidths, Density, IconZoom};
pub use command::Command;
pub use entry::{Entry, EntryKind, ExtId, IconId, SortDirection, SortKey, ViewMode};
pub use icon::RasterIcon;
pub use location::Location;
pub use naming::{BatchError, BatchSpec, NameError, batch_names, unique_name, validate};
pub use selection::Selection;
pub use shortcut::{Group, KeyCombo, ResolvedShortcut, ShortcutBinding, conflicts, looks_usable_as_shortcut, resolve};
pub use tab::{TabSnapshot, TabState};
pub use undo::{UNDO_LIMIT, UndoAction, UndoStack};
pub use tabs::{Tab, TabId, TabsState};
pub use util::format_size;
