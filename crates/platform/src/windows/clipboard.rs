//! 剪贴板文件列表读写（本次仅预留架构位）。
//!
//! **现状**：应用内的剪贴板已经落地，但它在 `fm-services::clipboard` 里，
//! 是一个纯内存的暂存区。本文件承载的是**跨进程互通**那一半（尚未实现）：
//! 它一旦完成，「在本程序里复制、到资源管理器里粘贴」才会成立。
//!
//! 规划接口：
//! ```ignore
//! pub trait Clipboard: Send + Sync + 'static {
//!     fn set_files(&mut self, paths: &[PathBuf]) -> anyhow::Result<()>;
//!     fn files(&mut self) -> anyhow::Result<Option<Vec<PathBuf>>>;
//!     fn clear(&mut self) -> anyhow::Result<()>;
//! }
//! ```
//! 实现要点：需要 `CF_HDROP` 格式，且写入后要调用 `OleSetClipboard`/`OleFlushClipboard`
//! 才能让资源管理器在进程退出后仍能粘贴。因此 `OleInitialize` 必须在主线程完成。
