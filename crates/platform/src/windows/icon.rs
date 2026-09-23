//! 系统图标提取。
//!
//! 规划实现：`SHGetFileInfoW`（或 `IShellItemImageFactory`）按路径取出
//! 小图标/大图标句柄，转换为 GPUI 可用的位图后交给 `fm-services` 的图标缓存持有。
//!
//! 生命周期提醒：`SHGetFileInfoW` 返回的 `HICON` 需要 `DestroyIcon` 释放；
//! 缓存需按扩展名维度做 LRU 上限，否则长目录滚动会持续泄漏 GDI 句柄。
