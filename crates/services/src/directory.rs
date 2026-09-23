//! 目录扫描流水线。
//!
//! 职责：把 `FileSystem::scan` 的原始流加工成 UI 可直接消费的有序批次，具体四件事：
//!
//! 1. **补全扩展名句柄**：为文件条目从驻留表换取 `ExtId`，供排序与详情视图的「类型」列使用；
//! 2. **排序**：O(n log n)，比较器只读已就绪字段，过程中不分配；
//! 3. **分批回传**：每批 [`DEFAULT_BATCH`] 条，让 UI 的每次更新都有明确上界；
//! 4. **generation 校验与取消**：代数一旦变化，在途扫描立刻放弃，连排序都省掉。
//!
//! 关于「首屏」的取舍：排序需要完整集合，因此本实现是「读全 → 排序 → 分批回传」，
//! 而不是「先回传未排序的首批、再整体重排」。后者会让列表在中途明显洗牌。
//! 真正的耗时大头是逐条 metadata 查询而非排序本身；若后续实测首屏仍不理想，
//! 再引入「未排序首批先行」的两阶段方案。
//!
//! 日志约定：只记录目录路径与耗时、条目数，**禁止**打印条目列表或目录内容。

use std::cmp::Ordering;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

use fm_core::{Entry, EntryKind, SortDirection, SortKey};
use fm_platform::FileSystem;

use crate::icon::{ExtInterner, extension_of};

/// 默认批次大小。
///
/// 512 是「首屏可见」与「单次更新成本」之间的折中：
/// 再小会让回传次数与重绘次数成倍上升，再大则单次排序前的等待变长。
pub const DEFAULT_BATCH: usize = 512;

/// 排序方式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SortSpec {
    pub key: SortKey,
    pub direction: SortDirection,
}

impl SortSpec {
    pub fn new(key: SortKey, direction: SortDirection) -> Self {
        Self { key, direction }
    }
}

/// 一批已排序的扫描结果。
#[derive(Debug)]
pub struct ScanBatch {
    /// 该批次所属的扫描代数——UI 收到后必须再比对一次，不一致就直接丢弃。
    pub generation: u64,
    /// 本批条目。
    pub entries: Vec<Entry>,
    /// 是否为该次扫描的最后一批（空目录也会产生一个 `is_last = true` 的空批次）。
    pub is_last: bool,
}

/// 目录扫描服务。
///
/// 单例使用：由 app 层构造一次，所有标签页共享同一个实例。
/// 共享的目的不只是省内存，更是为了让**扩展名句柄在全局唯一**。
pub struct DirectoryService {
    file_system: Arc<dyn FileSystem>,
    interner: ExtInterner,
    /// 当前有效的扫描代数。后台线程每批回传前比对此值。
    generation: AtomicU64,
}

impl DirectoryService {
    pub fn new(file_system: Arc<dyn FileSystem>) -> Self {
        Self {
            file_system,
            interner: ExtInterner::new(),
            generation: AtomicU64::new(0),
        }
    }

    /// 扩展名驻留表，供 UI 反查扩展名（详情视图的「类型」列、图标选择）。
    pub fn interner(&self) -> &ExtInterner {
        &self.interner
    }

    /// 底层文件系统能力，供 app 层读取驱动器与快速访问。
    pub fn file_system(&self) -> &Arc<dyn FileSystem> {
        &self.file_system
    }

    /// 开始一次新扫描并返回其代数。
    ///
    /// 该调用会立即让所有在途扫描失效——这正是「快速连续切换目录时旧结果不错乱」的保障。
    pub fn begin_scan(&self) -> u64 {
        self.generation.fetch_add(1, AtomicOrdering::SeqCst) + 1
    }

    /// 判断某个代数是否仍然是当前代数。
    pub fn is_current(&self, generation: u64) -> bool {
        self.generation.load(AtomicOrdering::SeqCst) == generation
    }

    /// 扫描目录，排序后分批回调。
    ///
    /// **本方法会阻塞**，必须在 `BackgroundExecutor` 上调用。
    /// `sink` 返回 `false` 表示调用方不再需要后续批次，届时立即返回。
    /// 若 `sink` 一直返回 `true`，最后一批的 `is_last` 为 `true`。
    pub fn scan(
        &self,
        dir: &Path,
        generation: u64,
        sort: SortSpec,
        batch: usize,
        sink: &mut dyn FnMut(ScanBatch) -> bool,
    ) -> anyhow::Result<()> {
        let batch = batch.max(1);
        let started = std::time::Instant::now();

        let mut entries: Vec<Entry> = Vec::new();
        self.file_system.scan(dir, batch, &mut |raw| {
            let mut raw = raw;
            for entry in &mut raw {
                attach_extension(&self.interner, entry);
            }
            entries.append(&mut raw);
            // 返回 false 会让 FileSystem 立即停止后续 IO：
            // 用户已经切到别的目录了，为一个没人看的目录继续扫描纯属浪费。
            self.is_current(generation)
        })?;

        // 代数已失效：整次扫描的结果一律丢弃，连排序都省掉。
        if !self.is_current(generation) {
            return Ok(());
        }

        sort_entries(&mut entries, sort);

        tracing::info!(
            directory = %dir.display(),
            entries = entries.len(),
            elapsed_ms = started.elapsed().as_millis() as u64,
            "目录扫描完成"
        );

        // 空目录也要回传一个空批次：否则 UI 无法区分「扫完了，是空的」与「还在扫」，
        // 会一直停在骨架屏上。
        if entries.is_empty() {
            sink(ScanBatch {
                generation,
                entries: Vec::new(),
                is_last: true,
            });
            return Ok(());
        }

        // 回传前逐批再查一次代数：用户可能在排序期间又切了目录。
        while !entries.is_empty() {
            if !self.is_current(generation) {
                return Ok(());
            }

            let take = batch.min(entries.len());
            let chunk: Vec<Entry> = entries.drain(..take).collect();
            let is_last = entries.is_empty();

            if !sink(ScanBatch {
                generation,
                entries: chunk,
                is_last,
            }) || is_last
            {
                return Ok(());
            }
        }

        Ok(())
    }
}

/// 为文件条目补上扩展名句柄。
///
/// 放在扫描阶段一次性完成，避免渲染路径上重复做小写化与字符串分配。
/// 目录、驱动器、符号链接没有扩展名概念，直接跳过。
fn attach_extension(interner: &ExtInterner, entry: &mut Entry) {
    let EntryKind::File { ext } = &mut entry.kind else {
        return;
    };
    if ext.is_some() {
        return;
    }
    if let Some(extension) = extension_of(&entry.name) {
        *ext = interner.intern(&extension);
    }
}

/// 按给定排序方式原地排序。
///
/// 两条约定：
/// - **目录永远排在文件前面**（与资源管理器一致），分组优先于排序键；
/// - 排序键相等时按名称做稳定的次级比较，避免同级条目顺序随机跳动。
fn sort_entries(entries: &mut [Entry], sort: SortSpec) {
    entries.sort_by(|a, b| {
        let group = group_rank(a.kind).cmp(&group_rank(b.kind));
        if group != Ordering::Equal {
            return group;
        }

        let ordering = match sort.key {
            SortKey::Name => cmp_name_insensitive(&a.name, &b.name),
            SortKey::Size => a.size.cmp(&b.size),
            // 类型相同再比名称：否则同扩展名的一堆文件每次排序顺序都可能不同。
            SortKey::Kind => a
                .ext()
                .cmp(&b.ext())
                .then_with(|| cmp_name_insensitive(&a.name, &b.name)),
            SortKey::Modified => a.modified.cmp(&b.modified),
        };

        let ordering = match sort.direction {
            SortDirection::Ascending => ordering,
            SortDirection::Descending => ordering.reverse(),
        };

        ordering.then_with(|| cmp_name_insensitive(&a.name, &b.name))
    });
}

/// 目录分组：0 表示目录，1 表示其余。
fn group_rank(kind: EntryKind) -> u8 {
    match kind {
        EntryKind::Directory => 0,
        _ => 1,
    }
}

/// 大小写不敏感的名称比较，**全程不分配**。
///
/// 直接在字符迭代器上比较，而不是先 `to_lowercase()` 生成新字符串：
/// 排序过程中比较次数是 O(n log n)，每次比较都分配一个字符串会让大目录的排序时间显著恶化，
/// 也会造成大量短命分配。
fn cmp_name_insensitive(a: &str, b: &str) -> Ordering {
    a.chars()
        .flat_map(char::to_lowercase)
        .cmp(b.chars().flat_map(char::to_lowercase))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fm_platform::mock::MockFileSystem;

    fn service(entries: usize) -> DirectoryService {
        DirectoryService::new(Arc::new(MockFileSystem::new(entries)))
    }

    fn collect(service: &DirectoryService, batch: usize) -> Vec<ScanBatch> {
        let generation = service.begin_scan();
        let mut batches = Vec::new();
        service
            .scan(
                Path::new(r"C:\mock"),
                generation,
                SortSpec::new(SortKey::Name, SortDirection::Ascending),
                batch,
                &mut |batch| {
                    batches.push(batch);
                    true
                },
            )
            .expect("mock 扫描不应失败");
        batches
    }

    #[test]
    fn results_are_chunked_and_last_batch_is_flagged() {
        let service = service(1000);
        let batches = collect(&service, DEFAULT_BATCH);

        let sizes: Vec<usize> = batches.iter().map(|batch| batch.entries.len()).collect();
        assert_eq!(sizes, vec![512, 488]);
        assert!(!batches[0].is_last);
        assert!(batches[1].is_last);
    }

    #[test]
    fn every_batch_carries_the_scan_generation() {
        let service = service(10);
        let generation = service.begin_scan();
        let mut seen = None;
        service
            .scan(
                Path::new(r"C:\mock"),
                generation,
                SortSpec::new(SortKey::Name, SortDirection::Ascending),
                DEFAULT_BATCH,
                &mut |batch| {
                    seen = Some(batch.generation);
                    true
                },
            )
            .unwrap();

        assert_eq!(seen, Some(generation));
    }

    #[test]
    fn stale_generation_discards_the_whole_scan() {
        let service = service(10);
        let stale = service.begin_scan();
        // 模拟用户在扫描开始后又切了一次目录。
        let _current = service.begin_scan();

        let mut batches = 0usize;
        service
            .scan(
                Path::new(r"C:\mock"),
                stale,
                SortSpec::new(SortKey::Name, SortDirection::Ascending),
                DEFAULT_BATCH,
                &mut |_| {
                    batches += 1;
                    true
                },
            )
            .unwrap();

        assert_eq!(batches, 0, "过期代数的批次必须全部丢弃");
    }

    #[test]
    fn directories_sort_before_files() {
        let service = service(8);
        let batches = collect(&service, DEFAULT_BATCH);
        let entries = &batches[0].entries;

        // mock 的命名规则：index % 4 == 0 为目录。
        assert!(entries[0].is_dir(), "首项必须是目录");
        let first_file = entries
            .iter()
            .position(|entry| !entry.is_dir())
            .expect("应当存在文件条目");
        assert!(
            entries[..first_file].iter().all(|entry| entry.is_dir()),
            "文件不得出现在目录之前"
        );
    }

    #[test]
    fn names_are_ordered_case_insensitively() {
        let mut entries = vec![
            entry("beta.txt"),
            entry("Alpha.txt"),
            entry("gamma.txt"),
        ];
        sort_entries(
            &mut entries,
            SortSpec::new(SortKey::Name, SortDirection::Ascending),
        );
        let names: Vec<&str> = entries.iter().map(|entry| &*entry.name).collect();
        assert_eq!(names, vec!["Alpha.txt", "beta.txt", "gamma.txt"]);
    }

    #[test]
    fn descending_reverses_names_but_keeps_directories_first() {
        let service = service(8);
        let generation = service.begin_scan();
        let mut batches = Vec::new();
        service
            .scan(
                Path::new(r"C:\mock"),
                generation,
                SortSpec::new(SortKey::Name, SortDirection::Descending),
                DEFAULT_BATCH,
                &mut |batch| {
                    batches.push(batch);
                    true
                },
            )
            .unwrap();

        let entries = &batches[0].entries;

        // 分组不受排序方向影响：降序时目录依然聚在最前面。
        let first_file = entries
            .iter()
            .position(|entry| !entry.is_dir())
            .expect("mock 应产生文件条目");
        assert!(
            entries[..first_file].iter().all(|entry| entry.is_dir()),
            "目录必须全部位于文件之前"
        );
        assert!(
            entries[first_file..].iter().all(|entry| !entry.is_dir()),
            "文件区不得混入目录"
        );

        // 排序方向确实生效：文件按名称降序，序号最大的排最前。
        assert_eq!(&*entries[first_file].name, "文件 7.txt");
    }

    #[test]
    fn empty_directory_still_emits_one_empty_batch() {
        let service = service(0);
        let batches = collect(&service, DEFAULT_BATCH);

        assert_eq!(batches.len(), 1, "空目录也必须回传一个批次");
        assert!(batches[0].entries.is_empty());
        assert!(batches[0].is_last);
    }

    #[test]
    fn file_entries_get_an_interned_extension() {
        let service = service(4);
        let batches = collect(&service, DEFAULT_BATCH);

        let file = batches[0]
            .entries
            .iter()
            .find(|entry| !entry.is_dir())
            .expect("mock 应产生文件条目");
        let ext = file.ext().expect("文件条目应已补全扩展名句柄");

        assert_eq!(service.interner().resolve(ext).as_deref(), Some("txt"));
    }

    #[test]
    fn sink_can_stop_the_stream_early() {
        let service = service(100_000);
        let generation = service.begin_scan();
        let mut delivered = 0usize;

        service
            .scan(
                Path::new(r"C:\mock"),
                generation,
                SortSpec::new(SortKey::Name, SortDirection::Ascending),
                DEFAULT_BATCH,
                &mut |batch| {
                    delivered += batch.entries.len();
                    false
                },
            )
            .unwrap();

        assert_eq!(delivered, DEFAULT_BATCH, "拒绝首批后不应再有产出");
    }

    fn entry(name: &str) -> Entry {
        Entry {
            name: Arc::from(name),
            path: Arc::new(std::path::PathBuf::from(name)),
            kind: EntryKind::File { ext: None },
            size: 0,
            modified: None,
            icon: fm_core::IconId(0),
        }
    }
}
