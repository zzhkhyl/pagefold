//! Windows 上的写操作：新建、改名、复制、移动。
//!
//! ## 为什么用 `std::fs` 而不是 `SHFileOperation` / `IFileOperation`
//!
//! 后者能免费拿到系统进度条与冲突对话框，听着很划算，代价却是**失去控制**：
//! `SHFileOperation` 只回一个错误码，而 UI 需要的是逐条回执（见 [`OpReport`]）——
//! 「已复制 3 项，1 项因占用失败」这句话它说不出来。
//! 真正非 shell 不可的只有一件事：**移入回收站**（见 [`super::recycle_bin`]）。
//! 因此这里的分工是「std 负责可控的部分，shell 只负责它独有的能力」。
//!
//! ## 长路径
//!
//! Rust 的 `std::fs` **不会**给路径加 `\\?\` 前缀，因此超过 260 字符的路径会直接
//! 失败。对文件管理器来说这不是边角情况（`node_modules` 里三层就两百多字符），
//! 所以本模块的每一次 IO 都经 [`extended`] 转换。

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result};
use fm_core::naming::unique_name;

use crate::fs::OpReport;

/// 在 `parent` 下新建目录，重名时自动改名。
pub fn create_dir(parent: &Path, name: &str) -> Result<PathBuf> {
    create_unique(parent, name, "文件夹", |path| fs::create_dir(path))
}

/// 在 `parent` 下新建空文件，重名时自动改名。
pub fn create_file(parent: &Path, name: &str) -> Result<PathBuf> {
    create_unique(parent, name, "文件", |path| {
        fs::File::create(path).map(|_| ())
    })
}

/// 改名。仅大小写不同的改名放行，其余重名报错。
pub fn rename(path: &Path, new_name: &str) -> Result<PathBuf> {
    let parent = path
        .parent()
        .with_context(|| format!("「{}」没有父目录，无法改名", path.display()))?;
    let target = parent.join(new_name);

    // 这一句是**必需**的，不是保险：Rust 的 `fs::rename` 在 Windows 上走
    // `MoveFileExW(MOVEFILE_REPLACE_EXISTING)`，也就是说**默认会静默覆盖**同名目标。
    // 不查一下，用户把 `新建文件夹` 改名为一个已存在的名字时会直接删掉别人的东西。
    //
    // 例外是仅大小写不同的改名（`报告.docx` → `报告.DOCX`）：那是修正命名，
    // 而且在同目录下 Windows 本身就不区分大小写，必须放行。
    let case_only = path
        .file_name()
        .is_some_and(|old| OsStr::new(new_name).eq_ignore_ascii_case(old));
    if !case_only && extended(&target).exists() {
        anyhow::bail!("「{new_name}」已存在");
    }

    fs::rename(extended(path), extended(&target))
        .with_context(|| format!("无法重命名为「{new_name}」"))?;
    Ok(target)
}

/// 复制若干条目进 `dest_dir`。
pub fn copy_into(sources: &[PathBuf], dest_dir: &Path) -> Result<OpReport> {
    let mut report = OpReport::default();

    for source in sources {
        match copy_one(source, dest_dir) {
            Ok((target, renamed)) => {
                report.done += 1;
                if renamed {
                    report.renamed.push((source.clone(), target));
                }
            }
            Err(error) => report.failed.push((source.clone(), describe(&error))),
        }
    }

    Ok(report)
}

/// 移动若干条目进 `dest_dir`。
pub fn move_into(sources: &[PathBuf], dest_dir: &Path) -> Result<OpReport> {
    let mut report = OpReport::default();

    for source in sources {
        match move_one(source, dest_dir) {
            Ok((target, renamed)) => {
                report.done += 1;
                if renamed {
                    report.renamed.push((source.clone(), target));
                }
            }
            Err(error) => report.failed.push((source.clone(), describe(&error))),
        }
    }

    Ok(report)
}

/// 新建条目并按需改名，返回最终路径。
fn create_unique(
    parent: &Path,
    desired: &str,
    kind: &str,
    create: impl Fn(&Path) -> io::Result<()>,
) -> Result<PathBuf> {
    let parent_ext = extended(parent);
    // 用「目标在不在」判断占用，而不是先列目录再比对：Windows 的文件系统本身
    // 不区分大小写，`exists()` 就是权威答案，而自己比对还得处理大小写与编码。
    let name = unique_name(desired, |candidate| parent_ext.join(candidate).exists());
    let path = parent.join(&name);

    create(&extended(&path)).with_context(|| format!("无法创建{kind}「{name}」"))?;
    Ok(path)
}

/// 复制单个条目，返回 `(实际目标路径, 是否因重名改过名)`。
fn copy_one(source: &Path, dest_dir: &Path) -> Result<(PathBuf, bool)> {
    let (target, renamed) = resolve_target(dest_dir, source)?;
    copy_tree(source, &target)?;
    Ok((target, renamed))
}

/// 移动单个条目。
fn move_one(source: &Path, dest_dir: &Path) -> Result<(PathBuf, bool)> {
    // 把目录移进它自己（或它的子目录）会自我吞食：`copy` 会一边写一边读、
    // 直到磁盘写满；`move` 更糟，删源时会把刚移进去的部分一起删掉。
    // 资源管理器同样拒绝这种操作。
    if source.is_dir() && dest_dir.starts_with(source) {
        anyhow::bail!("不能把文件夹移动到它自己里面");
    }

    let (target, renamed) = resolve_target(dest_dir, source)?;
    move_tree(source, &target)?;
    Ok((target, renamed))
}

/// 算出条目复制/移动到 `dest_dir` 后的路径，重名时自动改名。
fn resolve_target(dest_dir: &Path, source: &Path) -> Result<(PathBuf, bool)> {
    let name = source
        .file_name()
        .with_context(|| format!("无法确定「{}」的名称", source.display()))?
        .to_string_lossy()
        .into_owned();

    let dest_ext = extended(dest_dir);
    let resolved = unique_name(&name, |candidate| dest_ext.join(candidate).exists());
    let renamed = resolved != name;

    Ok((dest_dir.join(&resolved), renamed))
}

/// 递归复制。目录逐层新建，文件用 `fs::copy`（它会带上修改时间）。
fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(extended(from))
        .with_context(|| format!("无法读取「{}」", from.display()))?;

    // 符号链接与目录联接一律**拒绝**，而不是跟随。
    //
    // 跟随它们看起来更「完整」，但有两个真实后果：把同一棵树复抄多遍；
    // 遇到指向上级目录的联接（`node_modules` 与某些 junction 的常见形态）
    // 直接无限递归直到磁盘写满。资源管理器敢跟随是因为它有环检测与
    // 用户可选的冲突处理，这里没有，所以宁可明确报错。
    if metadata.is_symlink() {
        anyhow::bail!("不支持复制符号链接或目录联接");
    }

    if metadata.is_dir() {
        fs::create_dir(extended(to)).with_context(|| format!("无法创建目录「{}」", to.display()))?;
        let items = fs::read_dir(extended(from))
            .with_context(|| format!("无法读取目录「{}」", from.display()))?;
        for item in items {
            // 单个条目失败会让整项失败，不做「跳过继续」：复制留下的半成品
            // 比一次明确的失败更难收拾，而回执里会写明是哪一个条目出的问题。
            let item = item.with_context(|| format!("无法读取目录「{}」", from.display()))?;
            copy_tree(&item.path(), &to.join(item.file_name()))?;
        }
        return Ok(());
    }

    fs::copy(extended(from), extended(to)).with_context(|| format!("无法复制「{}」", from.display()))?;
    Ok(())
}

/// 移动。同卷用 `fs::rename`（瞬间完成、不产生副本），跨卷退化为复制 + 删源。
fn move_tree(from: &Path, to: &Path) -> Result<()> {
    match fs::rename(extended(from), extended(to)) {
        Ok(()) => Ok(()),
        Err(error) if is_cross_device(&error) => {
            // `rename` 不能跨卷。先复制再删源，中途失败会留下一份副本——
            // 这是所有文件管理器都接受的风险方向：宁可多留一份，也不能丢数据。
            copy_tree(from, to)?;
            remove_tree(from)?;
            Ok(())
        }
        Err(error) => {
            Err(error).with_context(|| format!("无法移动「{}」", from.display()))
        }
    }
}

/// 删除条目（移动的收尾）。文件与目录走不同的系统调用。
fn remove_tree(path: &Path) -> Result<()> {
    let metadata =
        fs::symlink_metadata(extended(path)).with_context(|| format!("无法读取「{}」", path.display()))?;
    if metadata.is_dir() {
        fs::remove_dir_all(extended(path))
            .with_context(|| format!("无法删除源目录「{}」", path.display()))
    } else {
        fs::remove_file(extended(path)).with_context(|| format!("无法删除源文件「{}」", path.display()))
    }
}

/// 该错误是否为「跨卷」。
///
/// Windows 返回 `ERROR_NOT_SAME_DEVICE`(17)。不按 `ErrorKind` 判断是因为
/// `ErrorKind::CrossesDevices` 在 Windows 上尚未稳定填写，而原始错误码是稳定的。
fn is_cross_device(error: &io::Error) -> bool {
    const ERROR_NOT_SAME_DEVICE: i32 = 17;
    error.raw_os_error() == Some(ERROR_NOT_SAME_DEVICE)
}

/// 给绝对路径加上 `\\?\` 前缀以突破 260 字符限制。
///
/// 三条必须遵守的规则，违反任何一条都会**让原本能用的路径失效**：
/// - 只处理**绝对**路径：`\\?\` 要求全限定路径；
/// - 路径里不能有 `.` / `..` 分量：加前缀后系统不再做规范化，
///   `..` 会被当成一个名字就叫「..」的目录；
/// - UNC 路径必须改写成 `\\?\UNC\server\share`，直接加前缀会得到无效路径。
pub(crate) fn extended(path: &Path) -> PathBuf {
    if !path.is_absolute() || path.components().any(needs_normalization) {
        return path.to_path_buf();
    }

    // 判断前缀可以用有损转换：这几个前缀字符都是 ASCII，而有损替换只会发生在
    // 非法 UTF-16 序列上——那不可能出现在路径开头。
    // 但**构造**结果绝不能走有损转换：Windows 允许文件名里含未配对代理项，
    // 替换成 U+FFFD 会指向另一个文件。因此一律用 `OsString::push` / `from_wide`。
    let text = path.as_os_str().to_string_lossy();
    if text.starts_with(r"\\?\") {
        return path.to_path_buf();
    }
    if text.starts_with(r"\\") {
        // 砍掉开头的两个反斜杠要按 UTF-16 码元来，不能用字符串切片。
        let rest: Vec<u16> = path.as_os_str().encode_wide().skip(2).collect();
        return prefixed(r"\\?\UNC\", &OsString::from_wide(&rest));
    }

    prefixed(r"\\?\", path.as_os_str())
}

/// 给路径加前缀，全程不做编码转换。
fn prefixed(prefix: &str, rest: &OsStr) -> PathBuf {
    let mut out = OsString::from(prefix);
    out.push(rest);
    PathBuf::from(out)
}

/// 该分量是否会让 `\\?\` 路径失去规范化能力。
fn needs_normalization(component: Component<'_>) -> bool {
    matches!(component, Component::CurDir | Component::ParentDir)
}

/// 把 IO / 上下文错误压成一句面向用户的话。
///
/// 回执里的失败原因直接展示给用户（toast），所以**不能**是
/// `Os { code: 5, kind: PermissionDenied, message: "拒绝访问。" }`。
/// 认不出的错误码退化为原始错误文本——那至少保留了可搜索的信息，
/// 比一句「操作失败」有用。
pub fn describe(error: &anyhow::Error) -> String {
    for cause in error.chain() {
        if let Some(io) = cause.downcast_ref::<io::Error>()
            && let Some(message) = describe_code(io.raw_os_error())
        {
            return message.to_string();
        }
    }

    // 走到这里说明是我们自己 `bail!` 出来的中文（如「不支持复制符号链接」），
    // 或者是认不出的错误码。两者都直接用最外层那句话。
    error.to_string()
}

/// 常见 Win32 错误码 → 用户可读的原因。
///
/// 只列真实会遇到的：权限、占用、空间、重名、路径过长。
fn describe_code(code: Option<i32>) -> Option<&'static str> {
    Some(match code? {
        2 => "项目不存在，可能已被移动或删除",
        3 => "路径不存在",
        5 => "没有权限，文件可能被占用或受保护",
        15 => "驱动器无效",
        19 => "目标磁盘被写保护",
        32 => "文件正被其它程序使用",
        33 => "文件的一部分被锁定",
        80 | 183 => "已存在同名项目",
        112 => "磁盘空间不足",
        123 => "名称不合法",
        145 => "目录不为空",
        206 => "路径过长",
        267 => "路径指向的不是目录",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extended_prefixes_absolute_paths() {
        assert_eq!(extended(Path::new(r"C:\a\b")), PathBuf::from(r"\\?\C:\a\b"));
        // 已经是扩展路径的一律不动，否则会变成 `\\?\UNC\?\C:\a`。
        assert_eq!(
            extended(Path::new(r"\\?\C:\a")),
            PathBuf::from(r"\\?\C:\a")
        );
    }

    #[test]
    fn extended_rewrites_unc_paths() {
        assert_eq!(
            extended(Path::new(r"\\server\share\dir")),
            PathBuf::from(r"\\?\UNC\server\share\dir"),
            "UNC 不能直接加前缀，否则得到一个无效路径"
        );
    }

    #[test]
    fn extended_leaves_relative_and_dotted_paths_alone() {
        // 相对路径不是全限定路径，加前缀后 `\\?\` 的语义不成立。
        assert_eq!(extended(Path::new(r"a\b")), PathBuf::from(r"a\b"));
        // 含 `..` 的分量在扩展路径里不会被规范化，会把「上一级」当成目录名。
        assert_eq!(
            extended(Path::new(r"C:\a\..\b")),
            PathBuf::from(r"C:\a\..\b"),
            "含 .. 的路径宁可保持原样（也许会因为长度失败，但不会指向错的地方）"
        );
    }

    #[test]
    fn describe_maps_common_error_codes() {
        let denied = anyhow::Error::new(io::Error::from_raw_os_error(5));
        assert!(describe(&denied).contains("没有权限"));

        let busy = anyhow::Error::new(io::Error::from_raw_os_error(32));
        assert!(describe(&busy).contains("正被其它程序使用"));

        let full = anyhow::Error::new(io::Error::from_raw_os_error(112));
        assert!(describe(&full).contains("空间不足"));
    }

    #[test]
    fn describe_finds_nested_io_errors() {
        // 真实路径上错误都被 `with_context` 包了一层，映射必须能穿透这层。
        let error = Err::<(), _>(io::Error::from_raw_os_error(183))
            .with_context(|| "无法复制「a.txt」")
            .unwrap_err();
        assert!(describe(&error).contains("已存在同名项目"));
    }

    #[test]
    fn describe_keeps_our_own_chinese_messages() {
        let error = anyhow::anyhow!("不支持复制符号链接或目录联接");
        assert_eq!(describe(&error), "不支持复制符号链接或目录联接");
    }

    #[test]
    fn describe_falls_back_to_the_raw_text() {
        // 认不出的错误码必须保住原始信息：那句英文至少是可搜索的。
        let error = anyhow::Error::new(io::Error::from_raw_os_error(4242));
        assert!(describe(&error).contains("4242"), "实际输出：{}", describe(&error));
    }
}

/// **真实 IO 的测试。**
///
/// 本文件其余测试都是纯函数（长路径前缀、错误码映射），而这一组真的动磁盘。
/// 理由是有些行为**只能靠真实调用验证**：改名到已存在的名字会不会静默覆盖、
/// 重名去重是不是真的加了序号、超过 `MAX_PATH` 的路径到底通不通。
/// 用 mock 验证这些等于自己确认自己。
///
/// 每个测试用自己的临时目录，结束时删掉。删除失败（文件被占用）不影响测试结果。
#[cfg(test)]
mod io_tests {
    use super::*;

    /// 一个独立的临时目录，离开作用域即清理。
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let mut path = std::env::temp_dir();
            // 进程号 + 纳秒 + 标签：并行跑的测试不能互相踩。
            path.push(format!(
                "fm-fsops-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|elapsed| elapsed.as_nanos())
                    .unwrap_or(0)
            ));
            fs::create_dir_all(&path).expect("无法创建临时目录");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn rename_refuses_an_existing_name() {
        let dir = TempDir::new("rename-collision");
        let source = dir.path().join("a.txt");
        let target = dir.path().join("b.txt");
        fs::write(&source, b"A").unwrap();
        fs::write(&target, b"B").unwrap();

        let result = rename(&source, "b.txt");

        assert!(result.is_err(), "改名到已存在的名字必须失败");
        // 这一条才是关键：Rust 的 `fs::rename` 在 Windows 上走
        // `MOVEFILE_REPLACE_EXISTING`，少了存在性检查，上一步会「成功」并把
        // b.txt 静默替换成 A 的内容——那是丢数据，不是改名。
        assert_eq!(fs::read(&target).unwrap(), b"B", "已存在的文件被覆盖了");
        assert_eq!(fs::read(&source).unwrap(), b"A", "源不该被动过");
    }

    #[test]
    fn rename_allows_a_case_only_change() {
        let dir = TempDir::new("rename-case");
        let source = dir.path().join("Report.txt");
        fs::write(&source, b"x").unwrap();

        let renamed = rename(&source, "REPORT.TXT").expect("仅大小写不同的改名应当放行");
        assert_eq!(renamed.file_name().unwrap(), "REPORT.TXT");
        assert_eq!(fs::read(&renamed).unwrap(), b"x", "内容不该丢");
    }

    #[test]
    fn create_dir_appends_a_number_on_collision() {
        let dir = TempDir::new("create-collision");
        fs::create_dir(dir.path().join("新建文件夹")).unwrap();

        let created = create_dir(dir.path(), "新建文件夹").expect("应当自动改名后创建成功");
        assert_eq!(created.file_name().unwrap(), "新建文件夹 (2)");
        assert!(created.is_dir());
    }

    #[test]
    fn create_file_makes_an_empty_file() {
        let dir = TempDir::new("create-file");
        let created = create_file(dir.path(), "新建文本文档.txt").expect("应当创建成功");
        assert_eq!(fs::metadata(&created).unwrap().len(), 0);
    }

    #[test]
    fn copy_into_renames_on_collision_and_keeps_the_original() {
        let dir = TempDir::new("copy-collision");
        let source_dir = dir.path().join("src");
        let dest_dir = dir.path().join("dst");
        fs::create_dir_all(&source_dir).unwrap();
        fs::create_dir_all(&dest_dir).unwrap();
        fs::write(source_dir.join("报告.docx"), b"new").unwrap();
        fs::write(dest_dir.join("报告.docx"), b"old").unwrap();

        let report = copy_into(&[source_dir.join("报告.docx")], &dest_dir).expect("不该整体失败");

        assert_eq!(report.done, 1);
        assert_eq!(report.renamed.len(), 1, "重名而改名的项必须记进回执");
        // 目标目录里原有的那份**不能**被动过。
        assert_eq!(fs::read(dest_dir.join("报告.docx")).unwrap(), b"old");
        assert_eq!(fs::read(dest_dir.join("报告 (2).docx")).unwrap(), b"new");
        assert!(
            source_dir.join("报告.docx").exists(),
            "复制必须保留源"
        );
    }

    #[test]
    fn move_into_moves_a_directory_with_its_contents() {
        let dir = TempDir::new("move-dir");
        let source = dir.path().join("src");
        let dest = dir.path().join("dst");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&dest).unwrap();
        fs::write(source.join("a.txt"), b"A").unwrap();

        let report = move_into(std::slice::from_ref(&source), &dest).expect("不该整体失败");

        assert_eq!(report.done, 1);
        assert!(!source.exists(), "源应当已经不在原处");
        assert_eq!(fs::read(dest.join("src").join("a.txt")).unwrap(), b"A");
    }

    #[test]
    fn move_into_refuses_to_move_a_directory_into_itself() {
        let dir = TempDir::new("move-into-self");
        let source = dir.path().join("src");
        let inner = source.join("inner");
        fs::create_dir_all(&inner).unwrap();

        let report = move_into(std::slice::from_ref(&source), &inner).expect("不该整体失败");

        assert_eq!(report.done, 0);
        let reason = report
            .failed
            .first()
            .map(|(_, reason)| reason.clone())
            .unwrap_or_default();
        assert!(
            reason.contains("自己"),
            "自我吞食必须被拦下并说明原因，实际：{reason}"
        );
    }

    #[test]
    fn copy_into_refuses_symlinks() {
        let dir = TempDir::new("copy-symlink");
        let source_dir = dir.path().join("src");
        let dest_dir = dir.path().join("dst");
        fs::create_dir_all(&source_dir).unwrap();
        fs::create_dir_all(&dest_dir).unwrap();
        fs::write(source_dir.join("real.txt"), b"x").unwrap();

        let link = source_dir.join("link.txt");
        if std::os::windows::fs::symlink_file(source_dir.join("real.txt"), &link).is_err() {
            // 建符号链接需要开发者模式或管理员权限。环境不允许就跳过——
            // 这不是被测代码的失败。
            return;
        }

        let report = copy_into(&[link], &dest_dir).expect("不该整体失败");

        assert_eq!(report.done, 0);
        let reason = report
            .failed
            .first()
            .map(|(_, reason)| reason.clone())
            .unwrap_or_default();
        assert!(
            reason.contains("符号链接"),
            "必须如实说明拒绝的原因，实际：{reason}"
        );
    }

    #[test]
    fn paths_beyond_max_path_work() {
        // 造一条超过 260 个 UTF-16 码元的目录链，再在它上面做真实的新建与复制。
        // 这是「长路径支持」唯一有意义的验证方式：不加深路径就测不到。
        let dir = TempDir::new("long-path");
        let mut deep = dir.path().to_path_buf();
        while deep.as_os_str().encode_wide().count() < 300 {
            deep.push("很长的目录名abcdefghij");
            fs::create_dir(extended(&deep)).expect("应当能建出超过 MAX_PATH 的目录");
        }

        let created = create_dir(&deep, "新建文件夹").expect("深路径上应当能新建");
        assert!(created.is_dir());
        assert!(created.as_os_str().encode_wide().count() > 260);

        // 在这条深路径上再放一个文件，然后复制回浅路径：
        // 读、写、复制三个方向都经过了 `\\?\` 转换。
        let file = create_file(&deep, "很长的文件名abcdefghij.txt").expect("应当能建文件");
        fs::write(extended(&file), b"deep").expect("应当能写进去");

        let report = copy_into(&[file], dir.path()).expect("不该整体失败");
        assert_eq!(
            report.failed,
            Vec::new(),
            "深路径的复制不该有失败项：{:?}",
            report.failed
        );
        assert_eq!(report.done, 1);
    }
}
