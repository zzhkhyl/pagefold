//! 剪贴板文件列表读写（本次仅预留架构位）。
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
