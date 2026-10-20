//! 系统图标（位图）的进程级注册表。
//!
//! ## 为什么是一个全局注册表
//!
//! 图标是**按扩展名共享**的资源：一屏 500 个 `.txt` 只需要一张 32×32 的位图。
//! 让 `Entry` 携带位图本身会把每条目撑大 4KB，而 `Entry` 的关键性质恰恰是
//! 「克隆只增加引用计数」。因此沿用 `Entry::icon: IconId` 这个句柄：
//! 分配句柄的一侧（`fm-services` 的图标缓存）把位图登记进这里，
//! 渲染的一侧（`fm-views`）用句柄取回。
//!
//! 走全局而不是层层传参，是因为位图**不参与布局与状态流转**：
//! 它没有「这一帧的和上一帧不同」的语义，也没有生命周期管理需求。
//! 为它给 `icon_grid::row` / `details_table::row` / `preview_panel::panel`
//! 各加一个参数，收益为零。
//!
//! ## 句柄约定
//!
//! - `IconId(0)` 是**保留值**，表示「没有系统图标，请用 SVG 兜底」。
//! - 分配从 1 开始，由调用方（`fm_services::icon::IconCache`）保证不重复。
//! - 注册是幂等的：同一个 id 重复注册只覆盖，不会 panic。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use crate::entry::IconId;

/// 一张系统图标位图（直通 alpha 的 RGBA8）。
///
/// 不做预乘：PNG 编码与 GPUI 的解码都按**直通** RGBA 处理，
/// 在这里做预乘只会让两边对不上。
#[derive(Clone, PartialEq, Eq)]
pub struct RasterIcon {
    pub width: u32,
    pub height: u32,
    /// 长度必须恰好是 `width * height * 4`。
    pub rgba: Vec<u8>,
}

impl RasterIcon {
    /// 由原始像素构造，长度不匹配时返回 `None`。
    ///
    /// 用 `Option` 而不是断言：位图来自系统 API，尺寸与缓冲区由平台层拼装，
    /// 一个算错的行距不该把整个程序带崩。
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Option<Self> {
        if width == 0 || height == 0 {
            return None;
        }
        let expected = (width as usize).checked_mul(height as usize)?.checked_mul(4)?;
        if rgba.len() != expected {
            return None;
        }
        Some(Self {
            width,
            height,
            rgba,
        })
    }

    /// 像素总数。
    pub fn pixels(&self) -> usize {
        self.width as usize * self.height as usize
    }
}

impl std::fmt::Debug for RasterIcon {
    /// 手写 `Debug`：派生会把几 KB 的像素全部打印出来，日志会立刻失控。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RasterIcon")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("bytes", &self.rgba.len())
            .finish()
    }
}

type Registry = Mutex<HashMap<IconId, Arc<RasterIcon>>>;

fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock() -> std::sync::MutexGuard<'static, HashMap<IconId, Arc<RasterIcon>>> {
    // 注册表只是缓存：某个线程持锁时 panic 不会让内容变得不自洽，
    // 因此中毒后继续用，而不是让整个渲染路径跟着崩。
    registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 登记一张位图。
pub fn register(id: IconId, icon: Arc<RasterIcon>) {
    if id == IconId(0) {
        // 0 是「没有系统图标」的保留值，不允许被占用。
        return;
    }
    lock().insert(id, icon);
}

/// 由句柄取回位图。未登记时返回 `None`（调用方退化为 SVG 图标）。
pub fn resolve(id: IconId) -> Option<Arc<RasterIcon>> {
    if id == IconId(0) {
        return None;
    }
    lock().get(&id).cloned()
}

/// 已登记的图标数量。供测试与诊断使用。
pub fn len() -> usize {
    lock().len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: u32, height: u32, value: u8) -> RasterIcon {
        RasterIcon::new(width, height, vec![value; (width * height * 4) as usize]).unwrap()
    }

    #[test]
    fn raster_icon_rejects_mismatched_buffers() {
        assert!(RasterIcon::new(2, 2, vec![0; 15]).is_none(), "少一个字节也应拒绝");
        assert!(RasterIcon::new(0, 2, Vec::new()).is_none());
        assert!(RasterIcon::new(2, 2, vec![0; 16]).is_some());
    }

    #[test]
    fn zero_id_is_reserved_and_never_resolves() {
        // `IconId(0)` 是「没有系统图标」的哨兵：注册也要被忽略，
        // 否则「没图标」与「第一张图标」会撞在一起。
        register(IconId(0), Arc::new(solid(1, 1, 1)));
        assert!(resolve(IconId(0)).is_none());
    }

    #[test]
    fn registering_then_resolving_round_trips() {
        let id = IconId(9_001);
        register(id, Arc::new(solid(2, 2, 7)));
        let icon = resolve(id).expect("刚登记的图标应能取回");
        assert_eq!(icon.width, 2);
        assert_eq!(icon.height, 2);
        assert_eq!(icon.rgba.len(), 16);
        assert!(resolve(IconId(9_002)).is_none());
    }
}
