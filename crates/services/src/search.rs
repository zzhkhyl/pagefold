//! 递归搜索：在当前目录及其子文件夹里按**文件名**匹配。
//!
//! ## 为什么不放在目录扫描里
//!
//! 普通列目录是「一层、几十到几千条」，而递归搜索是「一棵树、可能几十万条」。
//! 两者共用一条扫描通道会让「打开目录」也被搜索的体量拖慢。因此这里是独立的一条：
//! 自己的取消标志、自己的批量回传、自己的上限。
//!
//! ## 三条边界
//!
//! 1. **上限 2000 条**：用户要的是「找到那个文件」，不是「导出整个磁盘」。
//!    超过之后继续扫只是在烧 IO，界面也不会更好用。
//! 2. **不进入符号链接 / 目录联接**：与复制那边同理，一次误入指向上级的联接
//!    就会让搜索变成死循环。
//! 3. **跳过回收站与系统卷信息**：那两个目录要么读不到、要么内容没有意义，
//!    进一次只会换来一串「拒绝访问」。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use fm_core::{Entry, EntryKind, IconId};
use fm_platform::CancelToken;

/// 一次搜索最多回传多少条。
pub const MAX_RESULTS: usize = 2000;

/// 每批回传多少条。
///
/// 太小会让界面上「一行一行跳」，太大又让第一屏等太久。64 是两者之间比较舒服的点。
const BATCH_SIZE: usize = 64;

/// 后台搜索与界面之间的共享状态。
///
/// 后台线程只往里写，界面在 `ambient_tick` 里取走——与缩略图、系统剪贴板同一套模式。
#[derive(Default)]
pub struct SearchState {
    /// 还没被界面取走的命中条目。
    pub pending: Vec<Entry>,
    /// 后台线程是否已经收工。
    pub done: bool,
}

impl SearchState {
    /// 取走待处理的条目，并读取「是否已结束」。
    pub fn drain(&mut self) -> (Vec<Entry>, bool) {
        (std::mem::take(&mut self.pending), self.done)
    }
}

/// 搜索请求。
#[derive(Clone, Debug)]
pub struct Request {
    pub root: PathBuf,
    /// 已经小写化的关键词片段（调用方负责 trim）。
    pub needle: String,
    pub include_hidden: bool,
}

/// 递归搜索，按批回传结果，返回命中总数。
///
/// `sink` 每收到一批就被调用一次；它返回后搜索继续，因此界面可以边搜边显示。
pub fn search(request: &Request, cancel: &CancelToken, sink: &mut dyn FnMut(Vec<Entry>)) -> usize {
    let mut pending = Vec::with_capacity(BATCH_SIZE);
    let mut found = 0usize;

    walk(request, cancel, &mut |entry| {
        pending.push(entry);
        found += 1;

        if pending.len() >= BATCH_SIZE {
            sink(std::mem::take(&mut pending));
        }
        // 返回 false 表示「够了，别扫了」。
        found < MAX_RESULTS
    });

    if !pending.is_empty() {
        sink(pending);
    }
    found
}

/// 深度优先遍历，把命中的条目逐条交给 `visit`。
///
/// `visit` 返回 `false` 时立刻停止——上限与取消都靠它统一表达。
fn walk(request: &Request, cancel: &CancelToken, visit: &mut dyn FnMut(Entry) -> bool) {
    let mut stack = vec![request.root.clone()];

    while let Some(dir) = stack.pop() {
        if cancel.is_cancelled() {
            return;
        }
        let Ok(reader) = std::fs::read_dir(&dir) else {
            // 没权限的目录（系统卷信息、别的用户的配置目录）跳过即可，
            // 报错只会让一次搜索变成一串弹窗。
            continue;
        };

        for item in reader.flatten() {
            if cancel.is_cancelled() {
                return;
            }
            let Ok(metadata) = item.metadata() else {
                continue;
            };
            let path = item.path();
            let name = item.file_name().to_string_lossy().into_owned();
            let hidden = fm_platform::is_hidden(&metadata);

            if metadata.is_dir() {
                // 链接与联接一律不进入：一次误入指向上级的联接就是死循环。
                if metadata.file_type().is_symlink() || is_skipped_dir(&name) {
                    continue;
                }
                if hidden && !request.include_hidden {
                    continue;
                }
                stack.push(path.clone());
            }

            if hidden && !request.include_hidden {
                continue;
            }
            if !name.to_lowercase().contains(&request.needle) {
                continue;
            }

            if !visit(entry(path, name, &metadata, hidden)) {
                return;
            }
        }
    }
}

/// 回收站与系统卷信息：读不到，内容也没有搜索价值。
fn is_skipped_dir(name: &str) -> bool {
    name.eq_ignore_ascii_case("$Recycle.Bin") || name.eq_ignore_ascii_case("System Volume Information")
}

fn entry(path: PathBuf, name: String, metadata: &std::fs::Metadata, hidden: bool) -> Entry {
    Entry {
        name: Arc::from(name.as_str()),
        path: Arc::new(path),
        kind: if metadata.is_dir() {
            EntryKind::Directory
        } else {
            // 搜索不解析扩展名句柄：这里只是为了走同一条渲染路径，
            // 图标会在 `IconCache` 里按扩展名兜底。
            EntryKind::File { ext: None }
        },
        size: if metadata.is_dir() { 0 } else { metadata.len() },
        modified: metadata.modified().ok(),
        icon: IconId(0),
        hidden,
    }
}

/// 把结果按名称排序：搜索是「找东西」，用户需要一个稳定、可预期的顺序。
pub fn sort_by_name(entries: &mut [Entry]) {
    entries.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.path.cmp(&right.path))
    });
}

/// 相对路径（用于在结果里显示「它到底在哪一层」）。
pub fn relative_label(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(|rest| rest.display().to_string())
        .unwrap_or_else(|_| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pagefold-search-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("创建临时目录");
        dir
    }

    fn collect(root: &Path, needle: &str) -> Vec<Entry> {
        let request = Request {
            root: root.to_path_buf(),
            needle: needle.to_lowercase(),
            include_hidden: false,
        };
        let mut all = Vec::new();
        search(&request, &CancelToken::new(), &mut |batch| all.extend(batch));
        all
    }

    #[test]
    fn finds_matches_in_nested_directories() {
        let root = scratch("nested");
        let deep = root.join("a").join("b");
        fs::create_dir_all(&deep).expect("建目录");
        fs::write(root.join("report.txt"), b"1").expect("写入");
        fs::write(deep.join("Report-2026.txt"), b"2").expect("写入");
        fs::write(deep.join("other.md"), b"3").expect("写入");

        let hits = collect(&root, "report");

        assert_eq!(hits.len(), 2, "两层深的那份也要被找到");
        assert!(hits.iter().all(|entry| entry.name.to_lowercase().contains("report")));
        // 大小写不敏感：`Report-2026.txt` 与 `report.txt` 都算命中。
        assert!(hits.iter().any(|entry| entry.name.as_ref() == "Report-2026.txt"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn skipping_the_recycle_bin_and_system_volume_information() {
        let root = scratch("skip");
        let bin = root.join("$Recycle.Bin");
        fs::create_dir_all(&bin).expect("建目录");
        fs::write(bin.join("secret-report.txt"), b"x").expect("写入");

        let hits = collect(&root, "report");

        assert!(hits.is_empty(), "回收站目录不该被搜进来");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn cancellation_stops_the_walk() {
        let root = scratch("cancel");
        for index in 0..20 {
            fs::write(root.join(format!("hit-{index}.txt")), b"x").expect("写入");
        }

        let request = Request {
            root: root.clone(),
            needle: String::from("hit"),
            include_hidden: false,
        };
        let cancel = CancelToken::new();
        cancel.cancel();

        let mut all = Vec::new();
        let found = search(&request, &cancel, &mut |batch| all.extend(batch));

        assert_eq!(found, 0, "开始前就取消，不该有任何命中");
        assert!(all.is_empty());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn results_are_sorted_by_name() {
        let root = scratch("sort");
        for name in ["c.txt", "a.txt", "b.txt"] {
            fs::write(root.join(name), b"x").expect("写入");
        }

        let mut hits = collect(&root, ".txt");
        sort_by_name(&mut hits);

        let names: Vec<&str> = hits.iter().map(|entry| entry.name.as_ref()).collect();
        assert_eq!(names, vec!["a.txt", "b.txt", "c.txt"]);

        let _ = fs::remove_dir_all(&root);
    }

    /// 在**真实**目录树上跑一次。
    ///
    /// 默认 `#[ignore]`：耗时随机器与目录内容而定，不适合放进日常测试。
    /// 需要时手动跑：
    /// `cargo test -p fm-services -- --ignored searches_a_real_directory_tree`
    #[test]
    #[ignore = "遍历真实目录，耗时不定"]
    fn searches_a_real_directory_tree() {
        let root = PathBuf::from(r"C:\Windows\Web");
        if !root.exists() {
            return;
        }

        let request = Request {
            root: root.clone(),
            needle: String::from("img0"),
            include_hidden: true,
        };
        let mut hits = Vec::new();
        let found = search(&request, &CancelToken::new(), &mut |batch| hits.extend(batch));

        assert!(found >= 1, "系统自带的壁纸目录里应当有 img0.jpg");
        assert!(hits.iter().all(|entry| entry.name.to_lowercase().contains("img0")));
        // 每个结果都必须是真实存在的路径（拼路径时最容易出错的地方）。
        assert!(hits.iter().all(|entry| entry.path.exists()));
    }

    #[test]
    fn relative_label_strips_the_root() {
        let root = Path::new(r"C:\work");
        let nested = Path::new(r"C:\work\a\b.txt");

        assert_eq!(relative_label(root, nested), r"a\b.txt");
        // 不在根下的路径原样返回，不 panic。
        assert_eq!(
            relative_label(root, Path::new(r"D:\other.txt")),
            r"D:\other.txt"
        );
    }
}
