//! 拖出到其它程序（资源管理器、桌面、其它支持 `CF_HDROP` 的程序）。
//!
//! 平台侧的拖拽预览由系统自己绘制（文件图标 + 名字），因此这里只需要
//! 「带走哪些路径」以及每个路径是不是目录——后者是 GPUI 的
//! `FileDragPaths` 要求的：有了它，平台在拖拽开始就不必再 `stat` 每个路径。

use std::path::PathBuf;
use std::rc::Rc;

use fm_core::Entry;

/// 拖出时交给系统的文件列表：`(路径, 是否目录)`。
#[derive(Clone, Debug)]
pub struct DragOut(pub Rc<Vec<(PathBuf, bool)>>);

impl DragOut {
    /// 单个条目的载荷（拖未选中的行时用它）。
    pub fn single(entry: &Entry) -> Self {
        Self(Rc::new(vec![(
            entry.path.as_ref().clone(),
            entry.is_dir(),
        )]))
    }
}

/// 决定「拖这个条目时要带走哪些路径」。
///
/// 规则与资源管理器一致：拖**已选中**的行 → 带走整个选中集；
/// 拖未选中的行 → 只带走它自己。而「哪一批被选中」只有外壳层知道，
/// 因此做成回调，而不是把 `Selection` 塞进视图层。
pub type DragOutBuilder = Rc<dyn Fn(&Entry) -> DragOut>;

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, is_dir: bool) -> Entry {
        Entry {
            name: std::sync::Arc::from("x"),
            path: std::sync::Arc::new(PathBuf::from(path)),
            kind: if is_dir {
                fm_core::EntryKind::Directory
            } else {
                fm_core::EntryKind::File { ext: None }
            },
            size: 0,
            modified: None,
            icon: fm_core::IconId(0),
            hidden: false,
        }
    }

    #[test]
    fn single_keeps_the_path_and_the_directory_flag() {
        // 目录标记要交给平台：拖拽开始时它据此决定预览与落点语义，
        // 有了它平台就不必再对每个路径做一次 stat。
        let file = DragOut::single(&entry(r"C:\a\b.txt", false));
        assert_eq!(
            file.0.as_slice(),
            &[(PathBuf::from(r"C:\a\b.txt"), false)]
        );

        let folder = DragOut::single(&entry(r"C:\a", true));
        assert_eq!(folder.0.as_slice(), &[(PathBuf::from(r"C:\a"), true)]);
    }
}
