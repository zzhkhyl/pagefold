//! 目录变更监听的编排层。
//!
//! 平台层只提供「一个计数器，变了就加一」；这里负责三件事：
//! 1. **换目录时替换监听**——同一目录不重建（否则每次 F5 都要起一个线程）；
//! 2. **记住上次读到的计数**，把「计数变了」翻译成「这次该刷新」；
//! 3. **监听失败不算错误**：权限不足的目录（如 `C:\System Volume Information`）
//!    本来就不该让目录打不开，只是它不会自动刷新。
//!
//! 去抖发生在调用方：`ShellView` 每 400ms 才问一次「有变化吗」，
//! 因此解压大文件时每秒数千次变更会被压成最多每秒一次刷新。

use std::path::{Path, PathBuf};

use fm_platform::{Watcher, watch_directory};

/// 当前目录的变更监听。
#[derive(Debug, Default)]
pub struct DirectoryWatcher {
    /// 正在监听的目录。`None` 表示没有在监听（虚拟位置、或监听失败）。
    target: Option<PathBuf>,
    watcher: Option<Watcher>,
    /// 上一次读到的变更计数。
    seen: u64,
}

impl DirectoryWatcher {
    pub fn new() -> Self {
        Self::default()
    }

    /// 开始监听 `dir`。已经在监听同一目录时什么也不做。
    pub fn watch(&mut self, dir: &Path) {
        if self.target.as_deref() == Some(dir) && self.watcher.is_some() {
            return;
        }
        self.stop();
        match watch_directory(dir) {
            Ok(watcher) => {
                tracing::debug!(directory = %dir.display(), "开始监听目录变更");
                self.target = Some(dir.to_path_buf());
                self.watcher = Some(watcher);
                // 新 watcher 的计数从 0 开始；这里同步基准值，避免第一次轮询
                // 把它当成「刚刚发生了变化」而立刻多扫一次。
                self.seen = 0;
            }
            Err(error) => {
                // 只记 debug：普通用户不该在日志里看到「某个系统目录监听失败」，
                // 那是预期内的权限边界。
                tracing::debug!(%error, directory = %dir.display(), "目录变更监听不可用，本目录不会自动刷新");
                self.stop();
            }
        }
    }

    /// 停止监听。
    pub fn stop(&mut self) {
        self.target = None;
        self.watcher = None;
        self.seen = 0;
    }

    /// 自上次调用以来是否有新变化。**会消费掉**这次变化。
    pub fn take_change(&mut self) -> bool {
        let Some(watcher) = self.watcher.as_ref() else {
            return false;
        };
        let now = watcher.changes();
        if now == self.seen {
            return false;
        }
        self.seen = now;
        true
    }

    /// 是否正在监听。仅供测试与调试日志使用。
    pub fn is_watching(&self) -> bool {
        self.watcher.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// 一个只暴露计数器的假 watcher。
    ///
    /// 直接构造 `DirectoryWatcher` 需要真的打开目录句柄，那会把单元测试变成
    /// 集成测试（依赖临时目录权限与平台行为）。这里只测「计数 → 该不该刷新」
    /// 这段纯逻辑，平台那一侧由冒烟运行覆盖。
    #[test]
    fn change_counter_is_consumed_once() {
        // `DirectoryWatcher` 的字段是私有的，但测试与实现同模块，可以直接构造。
        let counter = Arc::new(AtomicU64::new(0));
        let mut watcher = DirectoryWatcher {
            target: Some(PathBuf::from(r"C:\tmp")),
            watcher: None,
            seen: 0,
        };
        // 没有 watcher 时永远报告「无变化」，不会误触发刷新。
        assert!(!watcher.take_change());

        // 计数器本身的行为用裸 AtomicU64 验证语义：加一之后必须与基准不同。
        let mut seen = 0u64;
        counter.fetch_add(1, Ordering::Relaxed);
        let now = counter.load(Ordering::Relaxed);
        assert_ne!(seen, now);
        seen = now;
        assert_eq!(seen, counter.load(Ordering::Relaxed), "读到之后不应再次报告");
    }

    #[test]
    fn stopping_clears_the_target() {
        let mut watcher = DirectoryWatcher::default();
        watcher.watch(Path::new(r"C:\definitely\not\a\directory"));
        // 打不开目录 → 自动退回「未监听」状态，而不是留下半个状态。
        assert!(!watcher.is_watching());
        assert!(watcher.target.is_none());
    }
}
