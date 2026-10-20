//! 扩展名驻留表（本次落地）与图标缓存（后续阶段）。
//!
//! ## 扩展名驻留表
//!
//! 扫描阶段把扩展名小写化后换取 `ExtId`（`u16`），收益有两处：
//! - `fm_core::Entry` 因此可以保持轻量，扩展名比较退化为整数比较；
//! - 排序与渲染路径上都不必重复做小写化与字符串分配。
//!
//! 句柄在进程内全局唯一，因此不同标签页之间也可以直接比较。
//!
//! ## 图标缓存（待落地）
//!
//! 规划：按**扩展名**（而非按路径）缓存图标并设 LRU 上限，
//! 使频繁滚动时命中为 O(1)、同类型文件共用同一张位图；
//! 系统图标句柄的提取由 `fm-platform` 的 `windows::icon` 负责。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use fm_core::{ExtId, IconId};

/// 扩展名驻留表。
///
/// 内部用 `Mutex` 而非 `RwLock`：写入只发生在「首次见到某个扩展名」时，
/// 而一整个目录里同类型文件的重复命中走的是读路径，临界区只有一次哈希查找，
/// 无竞争时 `Mutex` 的开销低于 `RwLock`。
#[derive(Debug, Default)]
pub struct ExtInterner {
    inner: Mutex<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    /// 扩展名 → 句柄。
    ids: HashMap<Arc<str>, ExtId>,
    /// 句柄 → 扩展名，用于详情视图的「类型」列。
    names: Vec<Arc<str>>,
}

impl ExtInterner {
    pub fn new() -> Self {
        Self::default()
    }

    /// 取扩展名的句柄，没有则登记一个新句柄。
    ///
    /// `ext` 必须是**已小写化且不含点**的扩展名（由 [`extension_of`] 产出）。
    /// 传入空串返回 `None`；扩展名种类超过 `u16::MAX` 时同样返回 `None`
    /// （此时放弃驻留，退化为「类型未知」而不是 panic）。
    pub fn intern(&self, ext: &str) -> Option<ExtId> {
        if ext.is_empty() {
            return None;
        }

        let mut inner = self.lock();
        if let Some(&id) = inner.ids.get(ext) {
            return Some(id);
        }

        let next = u16::try_from(inner.names.len()).ok()?;
        let key: Arc<str> = Arc::from(ext);
        inner.ids.insert(Arc::clone(&key), ExtId(next));
        inner.names.push(key);
        Some(ExtId(next))
    }

    /// 由句柄反查扩展名。返回 `Arc` 而非 `&str`，避免把锁的生命周期泄漏给调用方。
    pub fn resolve(&self, id: ExtId) -> Option<Arc<str>> {
        let inner = self.lock();
        inner.names.get(id.0 as usize).cloned()
    }

    /// 已登记的扩展名种类数。
    pub fn len(&self) -> usize {
        self.lock().names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 加锁并忽略中毒状态。
    ///
    /// 驻留表只是个纯缓存：即便某个线程在持锁时 panic，表内容仍然是自洽的，
    /// 没有理由因此让整个应用连带崩溃。
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// 系统图标缓存：**按扩展名**分配 `IconId`，位图登记进 `fm_core::icon`。
///
/// 为什么缓存在服务层而不是渲染层：`SHGetFileInfoW` 是 shell 调用（几毫秒），
/// 放在渲染路径上会让滚动掉帧；而扫描本来就在后台线程，顺带取图标正好。
///
/// 上限 512 种：正常情况下扩展名种类不过百，这个上限只为挡住「某个目录里
/// 全是随机扩展名」的异常输入。超限后新类型退化为 SVG 图标，不影响已有条目。
const MAX_ICONS: usize = 512;

#[derive(Debug, Default)]
pub struct IconCache {
    inner: Mutex<IconInner>,
}

#[derive(Debug, Default)]
struct IconInner {
    /// 缓存键（`dir` / `file:<ext>`）→ 句柄。`IconId(0)` 表示「取过，但系统没有」。
    ids: HashMap<String, IconId>,
    /// 下一个可用句柄。从 1 开始，0 是保留值。
    next: u32,
}

impl IconCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// 取某个扩展名（或文件夹）的图标句柄，必要时提取一次并缓存。
    ///
    /// 返回 `IconId(0)` 表示「没有系统图标，请用 SVG 兜底」。
    pub fn id_for(&self, extension: Option<&str>, directory: bool) -> IconId {
        let key = if directory {
            String::from("dir")
        } else {
            format!("file:{}", extension.unwrap_or_default())
        };

        let mut inner = self.lock();
        if let Some(&id) = inner.ids.get(&key) {
            return id;
        }
        if inner.ids.len() >= MAX_ICONS {
            tracing::debug!(limit = MAX_ICONS, "系统图标种类超过上限，后续类型退回内置图标");
            return IconId(0);
        }

        let id = match fm_platform::system_icon(extension, directory) {
            Some(raster) => {
                inner.next += 1;
                let id = IconId(inner.next);
                // 提取是低频事件（每种扩展名一次），但排查「图标为什么没出来」时
                // 这条日志是最直接的证据：它证明 shell 那一步成功了。
                tracing::debug!(
                    extension = extension.unwrap_or("<none>"),
                    directory,
                    width = raster.width,
                    height = raster.height,
                    "已提取系统图标"
                );
                fm_core::icon::register(id, Arc::new(raster));
                id
            }
            None => IconId(0),
        };
        // 失败也要记下来：否则每扫一次目录就重试一次 shell 调用。
        inner.ids.insert(key, id);
        id
    }

    /// 已缓存的种类数。
    pub fn len(&self) -> usize {
        self.lock().ids.len()
    }

    /// 是否尚未缓存任何图标。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, IconInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// 从文件名中提取小写扩展名。
///
/// 规则：
/// - 取最后一个点之后的部分；没有点、以点结尾、或以点开头（如 `.gitignore`）都视为无扩展名；
/// - 结果统一小写，使 `JPG` 与 `jpg` 命中同一个句柄。
pub fn extension_of(name: &str) -> Option<String> {
    let (stem, ext) = name.rsplit_once('.')?;
    if stem.is_empty() || ext.is_empty() {
        return None;
    }
    Some(ext.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_is_lowercased_and_last_segment_wins() {
        assert_eq!(extension_of("Report.TXT").as_deref(), Some("txt"));
        assert_eq!(extension_of("archive.tar.gz").as_deref(), Some("gz"));
    }

    #[test]
    fn names_without_a_real_extension_yield_none() {
        assert_eq!(extension_of("README"), None);
        assert_eq!(extension_of("trailing."), None);
        assert_eq!(extension_of(".gitignore"), None);
        assert_eq!(extension_of(""), None);
    }

    #[test]
    fn interning_is_stable_and_case_insensitive_upstream() {
        let interner = ExtInterner::new();
        let first = interner.intern("txt").expect("首次登记应成功");
        let second = interner.intern("txt").expect("重复登记应命中同一句柄");

        assert_eq!(first, second, "同一扩展名必须复用同一句柄");
        assert_eq!(interner.len(), 1);
        assert_eq!(interner.resolve(first).as_deref(), Some("txt"));
    }

    #[test]
    fn intern_assigns_distinct_ids_in_order() {
        let interner = ExtInterner::new();
        let txt = interner.intern("txt").unwrap();
        let png = interner.intern("png").unwrap();

        assert_ne!(txt, png);
        assert_eq!(txt, ExtId(0));
        assert_eq!(png, ExtId(1));
    }

    #[test]
    fn empty_extension_is_rejected() {
        let interner = ExtInterner::new();
        assert_eq!(interner.intern(""), None);
        assert!(interner.is_empty());
    }

    #[test]
    fn icon_ids_are_stable_per_extension() {
        // 首次调用可能真的去问 shell（Windows 上会返回一张位图），
        // 之后的调用必须是纯缓存命中：同一个扩展名拿到同一个句柄。
        let cache = IconCache::new();
        let first = cache.id_for(Some("txt"), false);
        let second = cache.id_for(Some("txt"), false);
        assert_eq!(first, second, "同一扩展名必须复用同一句柄");

        let dir = cache.id_for(None, true);
        assert_eq!(dir, cache.id_for(None, true));
        assert_eq!(cache.len(), 2, "两次不同键、各自一次缓存写入");
    }

    #[test]
    fn extensionless_files_and_directories_use_different_keys() {
        // 无扩展名文件与文件夹都可能落到「通用图标」，但它们必须是两条缓存，
        // 否则文件夹图标会被文件图标顶掉。
        let cache = IconCache::new();
        let file = cache.id_for(None, false);
        let dir = cache.id_for(None, true);
        assert_eq!(cache.len(), 2);
        // 两者是否指向同一张位图取决于系统；这里只断言缓存没有混淆。
        let file_again = cache.id_for(None, false);
        assert_eq!(file, file_again);
        let dir_again = cache.id_for(None, true);
        assert_eq!(dir, dir_again);
    }
}
