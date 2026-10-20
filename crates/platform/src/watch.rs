//! 目录变更监听：平台无关的入口。
//!
//! 契约只有一条：**监听线程不许直接改列表**。它只累加一个计数器，
//! 由 UI 侧的轮询决定何时刷新。这条约束把「外部程序每秒写几千个文件」
//! 与「渲染线程」彻底解耦——如果让每个事件都投递成一次重绘，
//! 解压一个压缩包就能把界面打死。
//!
//! 为什么用计数器而不是通道：`ReadDirectoryChangesW` 本身已经按批返回
//! （一次调用可能带回几十条记录），我们关心的是「有没有变化」，
//! 而不是「变化了几次」。一个 `AtomicU64` 就够，且不需要在 drop 时
//! 处理未读消息。
//!
//! 生命周期：`Watcher` 一被丢弃就停——`Drop` 里 `CancelIoEx` 打断阻塞中的
//! `ReadDirectoryChangesW`，线程随之退出并关闭目录句柄。因此上层只需
//! 「换目录时替换掉旧的 Watcher」，不需要额外的 stop 调用。

#[cfg(target_os = "windows")]
pub use crate::windows::dir_watch::{Watcher, watch_directory};

#[cfg(not(target_os = "windows"))]
use std::path::Path;

/// 非 Windows 宿主没有监听能力。
///
/// 这里给一个「永远不报告变化」的桩，而不是让调用点写 `#[cfg]`：
/// 目录监听是增强功能，缺了它程序照样能用（只是不自动刷新）。
#[cfg(not(target_os = "windows"))]
#[derive(Debug)]
pub struct Watcher;

#[cfg(not(target_os = "windows"))]
impl Watcher {
    /// 桩实现永远没有变化。
    pub fn changes(&self) -> u64 {
        0
    }
}

#[cfg(not(target_os = "windows"))]
pub fn watch_directory(_dir: &Path) -> anyhow::Result<Watcher> {
    anyhow::bail!("目录变更监听仅在 Windows 上可用")
}
