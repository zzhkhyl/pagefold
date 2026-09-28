//! 文件写操作的编排：校验名称、分派动作、把回执翻译成人话。
//!
//! 本层不碰平台 API（那是 `fm-platform` 的事），也不碰 UI（那是 `fm-shell` 的事）。
//! 它独立存在的理由是这里有三处判断，而三处都值得单测：
//!
//! 1. **提交前校验名称**：等磁盘报错就太晚了——用户要的不是
//!    `Os { code: 123, .. }`，而是「名称不能包含字符『?』」；
//! 2. **分派**：复制粘贴与剪切粘贴走的是两条不同的系统调用；
//! 3. **回执翻译**：`OpReport` 要变成「已复制 3 项，2 项因重名已改名」这样一句话。
//!
//! 与 `directory` 的分工：那边管「读」（扫描、分批、代数校验），
//! 这边管「写」。两者都不持有 UI 状态。

use std::path::{Path, PathBuf};

use fm_core::Entry;
use fm_core::naming::{self, NameError};
use fm_platform::{FileSystem, OpReport, describe_error};

use crate::clipboard::PastePlan;

/// 写操作的结果：失败时带一句**可直接展示**给用户的话。
///
/// 公开错误类型是 `String` 而不是 `anyhow::Error`，这是刻意的：
/// 这条链路的终点是 toast，任何上下文链到了那里都会被 `to_string()` 压成一行，
/// 不如在这里就压好——顺便保证它一定是一句中文。
pub type OpResult<T> = Result<T, String>;

/// 校验用户输入的名称，失败时给出可直接展示的原因。
pub fn check_name(name: &str) -> OpResult<String> {
    naming::validate(name)
        .map(str::to_string)
        .map_err(NameError::message)
}

/// 可新建的项目种类。
///
/// 做成枚举而不是让调用方各自传名字：默认名与「叫什么」是**一处知识**，
/// 菜单、工具栏下拉、空状态里的按钮三处都要用，分散写迟早会不一致。
/// 默认名的取值与设计稿的 `makeFolder()` / `makeFile('新建文本文档.txt','txt')`
/// / `makeFile('新建文档.md','md')` 三个调用一一对应。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NewItem {
    Folder,
    TextFile,
    MarkdownFile,
}

impl NewItem {
    /// 新建时使用的默认名称（重名时平台层会自动加序号）。
    pub fn default_name(self) -> &'static str {
        match self {
            Self::Folder => naming::DEFAULT_FOLDER,
            Self::TextFile => naming::DEFAULT_TEXT_FILE,
            Self::MarkdownFile => naming::DEFAULT_MARKDOWN_FILE,
        }
    }

}

/// 新建一个项目，返回它实际落在的路径（重名时自动改名）。
pub fn create(fs: &dyn FileSystem, dir: &Path, item: NewItem) -> OpResult<PathBuf> {
    match item {
        NewItem::Folder => create_folder(fs, dir, item.default_name()),
        NewItem::TextFile | NewItem::MarkdownFile => create_file(fs, dir, item.default_name()),
    }
}

/// 新建一个项目，并顺手把它**真实的元信息**读回来。
///
/// 为什么不等下一次目录扫描：新建之后要立刻把这一条显示在列表里——设计稿的
/// `makeFolder()` 是「建完立刻进入重命名」，而扫描回来可能要几百毫秒。
/// 顺手 `stat` 一次（同一条路径、紧接着的两次系统调用）比等一轮扫描便宜得多，
/// 而且拿到的是真元数据（大小、时间都由系统给出），不是我们拼出来的近似值。
pub fn create_with_entry(
    fs: &dyn FileSystem,
    dir: &Path,
    item: NewItem,
) -> OpResult<(PathBuf, Entry)> {
    let path = create(fs, dir, item)?;
    let entry = fs.stat(&path).map_err(|error| describe_error(&error))?;
    Ok((path, entry))
}

/// 新建文件夹，返回它实际落在的路径（重名时自动改名）。
pub fn create_folder(fs: &dyn FileSystem, dir: &Path, name: &str) -> OpResult<PathBuf> {
    let name = check_name(name)?;
    fs.create_dir(dir, &name)
        .map_err(|error| describe_error(&error))
}

/// 新建空文件，返回它实际落在的路径（重名时自动改名）。
pub fn create_file(fs: &dyn FileSystem, dir: &Path, name: &str) -> OpResult<PathBuf> {
    let name = check_name(name)?;
    fs.create_file(dir, &name)
        .map_err(|error| describe_error(&error))
}

/// 重命名。
pub fn rename(fs: &dyn FileSystem, path: &Path, new_name: &str) -> OpResult<PathBuf> {
    let new_name = check_name(new_name)?;

    // 名称没变就当作成功直接返回：用户点开改名、又原样回车是最常见的动作之一。
    // 交给系统调用没有意义（除了耗一次 IO），而且会撞上「目标已存在」那条检查——
    // 那会报出一个用户完全无法理解的「已存在」。
    if path.file_name().is_some_and(|old| old == new_name.as_str()) {
        return Ok(path.to_path_buf());
    }

    fs.rename(path, &new_name)
        .map_err(|error| describe_error(&error))
}

/// 执行一次粘贴。
///
/// 为什么由调用方先算 [`PastePlan`]：判定能不能粘贴（剪贴板空不空、
/// 是不是原位粘贴剪切内容）是纯逻辑，放在 `Clipboard` 里就能单测；
/// 这里只负责「照计划执行」，一行分支。
pub fn paste(fs: &dyn FileSystem, plan: &PastePlan) -> OpResult<OpReport> {
    let result = match plan {
        PastePlan::Copy { sources, dest_dir } => fs.copy_into(sources, dest_dir),
        PastePlan::Move { sources, dest_dir } => fs.move_into(sources, dest_dir),
    };
    result.map_err(|error| describe_error(&error))
}

/// 把若干条目移入回收站，返回实际处理的数量。
pub fn recycle(fs: &dyn FileSystem, paths: &[PathBuf]) -> OpResult<usize> {
    fs.move_to_recycle_bin(paths)
        .map_err(|error| describe_error(&error))
}

/// 把回执翻译成一句面向用户的话（成功与部分失败共用）。
///
/// 信息按「用户先关心哪个」排列：成功数量 → 改名 → 失败原因。
/// 反过来把失败放最前面确实更醒目，但 toast 的颜色（见调用方选择的
/// `Notification` 级别）已经在传递严重性了，文字再抢一次反而难读。
///
/// 失败原因**只列第一条**：toast 是一行字，罗列五个原因没人会看，
/// 而「有 N 项失败」+ 一个真实原因足以让用户知道该去哪里找问题。
/// 完整的失败列表留给调用方写日志。
pub fn summarize(verb: &str, report: &OpReport) -> String {
    let mut parts: Vec<String> = Vec::new();

    if report.done > 0 {
        parts.push(format!("已{verb} {} 项", report.done));
    }
    if !report.renamed.is_empty() {
        parts.push(format!("{} 项因重名已改名", report.renamed.len()));
    }
    if !report.failed.is_empty() {
        let (_, reason) = &report.failed[0];
        parts.push(format!("{} 项失败（{reason}）", report.failed.len()));
    }

    if parts.is_empty() {
        // 一条都没动、也没报错：源全都消失了才会走到这里。
        return format!("没有可{verb}的项");
    }

    parts.join("，")
}

#[cfg(test)]
mod tests {
    use super::*;
    use fm_platform::mock::{MockFileSystem, MockWrite};

    fn dir() -> PathBuf {
        PathBuf::from(r"C:\dir")
    }

    #[test]
    fn check_name_reports_the_reason_in_chinese() {
        assert_eq!(check_name("正常名称").unwrap(), "正常名称");
        // 断言的是「有解释」而不是具体措辞，避免把文案锁死在这里；
        // 具体措辞由 `fm_core::naming` 的测试负责。
        let reason = check_name("a?b").unwrap_err();
        assert!(reason.contains('?'), "原因里要指明是哪个字符：{reason}");
        assert!(check_name("   ").unwrap_err().contains("不能为空"));
    }

    #[test]
    fn create_folder_validates_before_touching_the_disk() {
        let fs = MockFileSystem::new(0);
        assert!(create_folder(&fs, &dir(), "a/b").is_err());
        assert!(
            fs.writes().is_empty(),
            "非法名称必须在提交前就被拦下，不能先调用系统再去报错"
        );

        create_folder(&fs, &dir(), "新建文件夹").unwrap();
        assert_eq!(
            fs.writes(),
            vec![MockWrite::CreateDir {
                parent: dir(),
                name: String::from("新建文件夹"),
            }]
        );
    }

    #[test]
    fn rename_skips_the_syscall_when_the_name_is_unchanged() {
        let fs = MockFileSystem::new(0);
        let path = dir().join("报告.docx");

        let result = rename(&fs, &path, "报告.docx").unwrap();
        assert_eq!(result, path);
        assert!(
            fs.writes().is_empty(),
            "名字没变时不该调系统调用——那会撞上「目标已存在」并报出莫名其妙的错"
        );
    }

    #[test]
    fn rename_still_checks_the_name_when_unchanged() {
        // 名称没变也要先校验：`""` 与原文不同才会走到系统调用，
        // 但 `" a? "` 这类必须先被拦下。
        let fs = MockFileSystem::new(0);
        let path = dir().join("a?b");
        assert!(
            rename(&fs, &path, "a?b").is_err(),
            "非法名称即便「没变」也不能放行"
        );
    }

    #[test]
    fn paste_dispatches_copy_and_cut_to_different_calls() {
        let fs = MockFileSystem::new(0);
        let sources = vec![dir().join("x.txt")];
        let dest = PathBuf::from(r"C:\other");

        paste(
            &fs,
            &PastePlan::Copy {
                sources: sources.clone(),
                dest_dir: dest.clone(),
            },
        )
        .unwrap();
        paste(
            &fs,
            &PastePlan::Move {
                sources: sources.clone(),
                dest_dir: dest.clone(),
            },
        )
        .unwrap();

        assert_eq!(
            fs.writes(),
            vec![
                MockWrite::Copy {
                    sources: sources.clone(),
                    dest_dir: dest.clone(),
                },
                MockWrite::Move {
                    sources,
                    dest_dir: dest,
                },
            ],
            "复制粘贴走 copy、剪切粘贴走 move，不能混"
        );
    }

    #[test]
    fn recycle_forwards_every_path() {
        let fs = MockFileSystem::new(0);
        let paths = vec![dir().join("a.txt"), dir().join("b.txt")];
        assert_eq!(recycle(&fs, &paths).unwrap(), 2);
        assert_eq!(fs.writes(), vec![MockWrite::Recycle { paths }]);
    }

    #[test]
    fn summarize_covers_success_rename_and_failure() {
        let clean = OpReport {
            done: 3,
            ..OpReport::default()
        };
        assert_eq!(summarize("复制", &clean), "已复制 3 项");

        let renamed = OpReport {
            done: 3,
            renamed: vec![(dir().join("a"), dir().join("a (2)"))],
            failed: Vec::new(),
        };
        assert_eq!(summarize("复制", &renamed), "已复制 3 项，1 项因重名已改名");

        let partial = OpReport {
            done: 2,
            renamed: Vec::new(),
            failed: vec![
                (dir().join("a"), String::from("文件正被其它程序使用")),
                (dir().join("b"), String::from("没有权限，文件可能被占用或受保护")),
            ],
        };
        let text = summarize("移动", &partial);
        assert_eq!(text, "已移动 2 项，2 项失败（文件正被其它程序使用）");
        assert!(
            !text.contains("没有权限"),
            "只报第一条失败原因，避免 toast 变成一长串：{text}"
        );
    }

    #[test]
    fn summarize_handles_a_report_where_nothing_happened() {
        assert_eq!(
            summarize("复制", &OpReport::default()),
            "没有可复制的项"
        );
    }

    #[test]
    fn create_dispatches_folder_and_file_to_different_calls() {
        let fs = MockFileSystem::new(0);
        let dir = dir();

        create(&fs, &dir, NewItem::Folder).unwrap();
        create(&fs, &dir, NewItem::TextFile).unwrap();
        create(&fs, &dir, NewItem::MarkdownFile).unwrap();

        assert_eq!(
            fs.writes(),
            vec![
                MockWrite::CreateDir {
                    parent: dir.clone(),
                    name: String::from("新建文件夹"),
                },
                MockWrite::CreateFile {
                    parent: dir.clone(),
                    name: String::from("新建文本文档.txt"),
                },
                MockWrite::CreateFile {
                    parent: dir.clone(),
                    name: String::from("新建文档.md"),
                },
            ]
        );
    }

    #[test]
    fn create_with_entry_returns_metadata_for_the_path_it_created() {
        let fs = MockFileSystem::new(0);
        let dir = dir();

        let (path, entry) = create_with_entry(&fs, &dir, NewItem::TextFile).unwrap();

        assert_eq!(path, dir.join("新建文本文档.txt"));
        assert_eq!(
            entry.path.as_ref(),
            &path,
            "元信息必须对应刚建出来的那一条，否则列表里会插进一个指向别处的条目"
        );
        // 记录里只有「建」这一步：`stat` 不是写操作，不该出现在写日志里。
        assert_eq!(
            fs.writes(),
            vec![MockWrite::CreateFile {
                parent: dir,
                name: String::from("新建文本文档.txt"),
            }]
        );
    }

    #[test]
    fn new_item_default_names_match_the_design() {
        // 设计稿的 `makeFile('新建文本文档.txt','txt')` 与 `makeFile('新建文档.md','md')`。
        assert_eq!(NewItem::Folder.default_name(), "新建文件夹");
        assert_eq!(NewItem::TextFile.default_name(), "新建文本文档.txt");
        assert_eq!(NewItem::MarkdownFile.default_name(), "新建文档.md");
    }
}
