//! 缩略图的异步队列。
//!
//! ## 为什么必须异步
//!
//! 取一张缩略图要真的解码（几十毫秒，视频还要抽帧）。目录里几百张照片时，
//! 在渲染路径上现取等于把界面冻住；在扫描线程里顺序取则会让「列目录」变成
//! 「先解码所有图片」。因此：**请求与渲染解耦**——渲染先用扩展名图标，
//! 取到之后再把那一格的图标换成缩略图。
//!
//! ## 为什么按白名单挑选
//!
//! `IShellItemImageFactory` 对**没有**预览处理器的文件会退回它的图标
//! （见 `fm_platform::thumbnail` 的测试）。如果对所有文件都发请求，
//! 得到的只是一堆高分辨率图标：既浪费解码与内存，观感还不如现有的、
//! 按扩展名去重过的小图标。因此只对「确实可能有内容预览」的类型发请求。
//!
//! ## 队列与背压
//!
//! 一个工作线程 + 一条无界队列。去重靠 `requested` 集合：同一个路径只请求一次，
//! 失败也记下来（否则每次滚动都会重试一批注定失败的路径）。
//! 进度通过 [`ThumbnailService::take_ready`] 轮询取走——外壳在 `ambient_tick`
//! 里每 400ms 收一次，与目录监听的节奏一致，不需要额外的唤醒通道。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};

use fm_core::IconId;

/// 判定「这个文件值不值得请求缩略图」。
///
/// 只列**确实有内容预览**的类型。刻意不包含 exe/lnk/文件夹：
/// 那些的「缩略图」就是它们自己的图标，请求了反而更糊。
pub fn is_previewable(extension: Option<&str>) -> bool {
    matches!(
        extension,
        Some(
            "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "ico" | "heic" | "tif" | "tiff"
                | "mp4" | "mkv" | "avi" | "mov" | "wmv" | "webm" | "m4v"
                | "pdf" | "svg"
        )
    )
}

/// 缩略图队列。克隆共享同一个工作线程与状态。
#[derive(Clone)]
pub struct ThumbnailService {
    sender: Sender<(PathBuf, u32)>,
    shared: Arc<Shared>,
}

#[derive(Default)]
struct Shared {
    /// 已经请求过的路径（成功与失败都算），用于去重与防重试。
    requested: Mutex<HashSet<PathBuf>>,
    /// 取到待交付的结果：`(路径, 句柄)`。
    ready: Mutex<Vec<(PathBuf, IconId)>>,
}

impl Shared {
    fn lock_requested(&self) -> std::sync::MutexGuard<'_, HashSet<PathBuf>> {
        self.requested
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn lock_ready(&self) -> std::sync::MutexGuard<'_, Vec<(PathBuf, IconId)>> {
        self.ready
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl ThumbnailService {
    /// 起一个工作线程。
    ///
    /// 单线程是刻意的：缩略图是**背景美化**，不该和用户的实际操作抢 CPU 与磁盘。
    /// 请求按目录顺序到达，于是视觉上是从上往下逐格出现，而不是随机闪。
    pub fn new() -> Self {
        let (sender, receiver) = channel::<(PathBuf, u32)>();
        let shared = Arc::new(Shared::default());

        let worker_shared = Arc::clone(&shared);
        let spawned = std::thread::Builder::new()
            .name(String::from("pagefold-thumbnail"))
            .spawn(move || {
                for (path, size) in receiver {
                    let id = match fm_platform::thumbnail(&path, size) {
                        Some(raster) => {
                            // 排查「缩略图为什么没出来」时，这条日志能直接区分
                            // 「没请求」「请求了但 shell 没给」「给了但没画出来」。
                            tracing::debug!(
                                path = %path.display(),
                                size,
                                width = raster.width,
                                height = raster.height,
                                "已生成缩略图"
                            );
                            fm_core::icon::register_raster(Arc::new(raster))
                        }
                        // 取不到就记 0：外壳据此知道「这条别再问了」。
                        None => {
                            tracing::debug!(path = %path.display(), "没有可用的缩略图，保留扩展名图标");
                            IconId(0)
                        }
                    };
                    worker_shared.lock_ready().push((path, id));
                }
            });

        if let Err(error) = spawned {
            // 线程起不来不该让程序崩：缩略图只是美化，退化成「永远只有图标」。
            tracing::warn!(%error, "缩略图线程启动失败，将只显示扩展名图标");
        }

        Self { sender, shared }
    }

    /// 请求一张缩略图。重复请求（同一路径）会被忽略。
    pub fn request(&self, path: &Path, size: u32) {
        if !self.shared.lock_requested().insert(path.to_path_buf()) {
            return;
        }
        // 发送失败说明工作线程已经退出（程序正在关闭）：忽略即可。
        let _ = self.sender.send((path.to_path_buf(), size));
    }

    /// 取走已完成的结果。外壳在 `ambient_tick` 里调用。
    pub fn take_ready(&self) -> Vec<(PathBuf, IconId)> {
        std::mem::take(&mut *self.shared.lock_ready())
    }
}

impl Default for ThumbnailService {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_content_types_are_previewable() {
        // 有内容预览的
        for ext in ["png", "jpg", "mp4", "pdf", "svg"] {
            assert!(is_previewable(Some(ext)), "{ext} 应当请求缩略图");
        }
        // 只会有图标、没有内容预览的
        for ext in ["txt", "exe", "lnk", "dll", "zip", "docx"] {
            assert!(!is_previewable(Some(ext)), "{ext} 不该请求缩略图");
        }
        assert!(!is_previewable(None), "无扩展名不该请求");
    }

    #[test]
    fn the_same_path_is_requested_only_once() {
        let service = ThumbnailService::new();
        let path = std::path::PathBuf::from(r"C:\pictures\a.png");

        service.request(&path, 128);
        service.request(&path, 128);
        service.request(&path, 256);

        // 三次调用只登记一次：`requested` 里有且只有这一条。
        assert_eq!(service.shared.lock_requested().len(), 1);
    }

    #[test]
    fn results_are_drained_once() {
        let service = ThumbnailService::new();
        service
            .shared
            .lock_ready()
            .push((PathBuf::from(r"C:\a.png"), IconId(7)));

        let first = service.take_ready();
        assert_eq!(first.len(), 1);
        assert!(
            service.take_ready().is_empty(),
            "取走之后必须清空，否则外壳会反复重放同一批结果"
        );
    }
}
