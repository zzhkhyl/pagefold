//! 应用内的文件剪贴板：复制 / 剪切的暂存区。
//!
//! ## 为什么先做应用内，而不是直接写系统剪贴板
//!
//! 真正的做法是往系统剪贴板写 `CF_HDROP`（`fm-platform/src/windows/clipboard.rs`
//! 里记着这条路线），好处是能与资源管理器互通：在本程序里复制、到资源管理器粘贴。
//! 但那需要自己实现一个 `IDataObject` 并正确配对 `OleSetClipboard` /
//! `OleFlushClipboard`，是一整块 COM 代码，而且**写错了不会报错**——
//! 只会表现为「切到资源管理器按粘贴没反应」，极难定位。
//!
//! 因此先做的是应用内剪贴板：**语义是完整的**（复制、剪切、粘贴、
//! 冲突自动改名、剪切后失效都在），缺的只是跨进程互通。
//!
//! 互通已经落地（`fm-platform/src/windows/clipboard.rs`），做法比预想的简单：
//! 不实现 `IDataObject`，直接写 `CF_HDROP`。本类型仍然是**语义的唯一来源**——
//! `ShellView` 在 `put` 之后把同一批路径同步写进系统剪贴板，
//! 粘贴时应用内优先、为空才回退去读系统剪贴板。

use std::path::{Path, PathBuf};

use fm_platform::OpReport;

/// 剪贴板里的操作意图。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipboardMode {
    /// 复制：粘贴后源保留，剪贴板继续有效。
    Copy,
    /// 剪切：粘贴成功后源被移走，剪贴板随之失效。
    Cut,
}

impl ClipboardMode {
    /// 放进剪贴板时的文案：「已复制 3 项到剪贴板」。
    pub fn verb(self) -> &'static str {
        match self {
            Self::Copy => "复制",
            Self::Cut => "剪切",
        }
    }

    /// 粘贴时的动作名：「复制 3 项」/「移动 3 项」。
    ///
    /// 与 [`ClipboardMode::verb`] 分开，是因为用户视角不同：
    /// 剪切说的是「我做了什么」，移动说的是「它正在发生什么」。
    pub fn action(self) -> &'static str {
        match self {
            Self::Copy => "复制",
            Self::Cut => "移动",
        }
    }
}

/// 这次粘贴该执行什么。
///
/// 做成一个可比较的枚举而不是直接执行，是为了让「判定」与「执行」分开：
/// 前者是纯逻辑（可单测），后者要动磁盘。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PastePlan {
    Copy {
        sources: Vec<PathBuf>,
        dest_dir: PathBuf,
    },
    Move {
        sources: Vec<PathBuf>,
        dest_dir: PathBuf,
    },
}

impl PastePlan {
    /// 目标目录。
    pub fn dest_dir(&self) -> &Path {
        match self {
            Self::Copy { dest_dir, .. } | Self::Move { dest_dir, .. } => dest_dir,
        }
    }

    /// 待处理的条目。
    pub fn sources(&self) -> &[PathBuf] {
        match self {
            Self::Copy { sources, .. } | Self::Move { sources, .. } => sources,
        }
    }
}

/// 为什么这次不能粘贴。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PasteError {
    /// 剪贴板是空的。
    Empty,
    /// 在**原位**粘贴剪切的内容。
    SameLocation,
}

impl PasteError {
    /// 面向用户的说明（可直接进 toast）。
    ///
    /// 两种情况的措辞都来自设计稿的 `paste()`：
    /// 「剪贴板为空」与「目标位置与源位置相同」。
    pub fn message(self) -> &'static str {
        match self {
            Self::Empty => "剪贴板为空",
            Self::SameLocation => "目标位置与源位置相同",
        }
    }
}

/// 应用内剪贴板。
///
/// 用 `Default` 表示「空」：`ShellView` 直接持有一个，不需要 `Option` 包一层。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Clipboard {
    /// 空表即为「没有内容」，因此不需要额外的 `mode` 字段来表达空态。
    content: Option<Content>,
}

/// 剪贴板的实际内容。
#[derive(Clone, Debug, PartialEq, Eq)]
struct Content {
    mode: ClipboardMode,
    sources: Vec<PathBuf>,
    /// 复制 / 剪切发起时所在的目录。
    ///
    /// 只为拦一件事：在**原位**粘贴剪切的内容。设计稿的 `paste()` 明确拦了它
    /// （`目标位置与源位置相同`），而那是真实会发生的误操作——`Ctrl+X` 之后
    /// 手一抖又按了 `Ctrl+V`，不拦就会平白得到一堆 `x (2)` 副本。
    origin: PathBuf,
}

impl Clipboard {
    /// 空剪贴板。
    pub fn new() -> Self {
        Self::default()
    }

    /// 放入一批条目。
    ///
    /// `sources` 为空时视为清空：调用方本该先拦住空选中，但真发生了也不该
    /// 留下一个「有剪贴板、但没有条目」的状态——那会让粘贴菜单项莫名其妙地可用。
    pub fn put(&mut self, mode: ClipboardMode, sources: Vec<PathBuf>, origin: PathBuf) {
        self.content = (!sources.is_empty()).then_some(Content {
            mode,
            sources,
            origin,
        });
    }

    /// 清空。
    pub fn clear(&mut self) {
        self.content = None;
    }

    /// 是否为空。UI 用它决定「粘贴」菜单项是否禁用。
    pub fn is_empty(&self) -> bool {
        self.content.is_none()
    }

    /// 内容的条目数（空时为 0）。
    pub fn len(&self) -> usize {
        self.content
            .as_ref()
            .map_or(0, |content| content.sources.len())
    }

    /// 剪贴板的意图（空时为 `None`）。
    pub fn mode(&self) -> Option<ClipboardMode> {
        self.content.as_ref().map(|content| content.mode)
    }

    /// 剪贴板里的条目（空时为空切片）。
    ///
    /// 需要它来做一件事：**在列表里把被剪切的条目画成半透明**
    /// （资源管理器的做法）。本轮尚未接这个视觉，但接口先留出来。
    pub fn sources(&self) -> &[PathBuf] {
        self.content
            .as_ref()
            .map_or(&[], |content| content.sources.as_slice())
    }

    /// 判定这次粘贴能不能做、该做什么。
    pub fn plan(&self, dest_dir: &Path) -> Result<PastePlan, PasteError> {
        let Some(content) = self.content.as_ref() else {
            return Err(PasteError::Empty);
        };

        if content.mode == ClipboardMode::Cut && dest_dir == content.origin {
            return Err(PasteError::SameLocation);
        }

        Ok(match content.mode {
            ClipboardMode::Copy => PastePlan::Copy {
                sources: content.sources.clone(),
                dest_dir: dest_dir.to_path_buf(),
            },
            ClipboardMode::Cut => PastePlan::Move {
                sources: content.sources.clone(),
                dest_dir: dest_dir.to_path_buf(),
            },
        })
    }

    /// 粘贴执行完之后收尾。
    ///
    /// 剪切的剪贴板**只在全部成功时**才清空：部分失败时留着重试，
    /// 比让用户再回去选一遍、再按一次剪切要好。复制的剪贴板永远保留
    /// （那是它存在的意义：粘十次）。
    pub fn complete(&mut self, plan: &PastePlan, report: &OpReport) {
        if let PastePlan::Move { .. } = plan
            && report.done > 0
            && !report.has_failures()
        {
            self.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(text: &str) -> PathBuf {
        PathBuf::from(text)
    }

    fn copied() -> Clipboard {
        let mut clipboard = Clipboard::new();
        clipboard.put(
            ClipboardMode::Copy,
            vec![path(r"C:\a\x.txt")],
            path(r"C:\a"),
        );
        clipboard
    }

    #[test]
    fn a_new_clipboard_is_empty() {
        let clipboard = Clipboard::new();
        assert!(clipboard.is_empty());
        assert_eq!(clipboard.len(), 0);
        assert_eq!(clipboard.mode(), None);
        assert_eq!(clipboard.plan(&path(r"C:\b")), Err(PasteError::Empty));
    }

    #[test]
    fn copying_yields_a_copy_plan_that_keeps_the_clipboard() {
        let mut clipboard = copied();
        let plan = clipboard.plan(&path(r"C:\b")).expect("复制粘贴应当可行");
        assert_eq!(
            plan,
            PastePlan::Copy {
                sources: vec![path(r"C:\a\x.txt")],
                dest_dir: path(r"C:\b"),
            }
        );

        clipboard.complete(&plan, &OpReport { done: 1, ..Default::default() });
        assert!(!clipboard.is_empty(), "复制之后剪贴板要留着，否则只能粘一次");
    }

    #[test]
    fn cutting_yields_a_move_plan_that_clears_the_clipboard_when_done() {
        let mut clipboard = Clipboard::new();
        clipboard.put(ClipboardMode::Cut, vec![path(r"C:\a\x.txt")], path(r"C:\a"));

        let plan = clipboard.plan(&path(r"C:\b")).expect("剪切粘贴应当可行");
        assert!(matches!(plan, PastePlan::Move { .. }));

        clipboard.complete(&plan, &OpReport { done: 1, ..Default::default() });
        assert!(clipboard.is_empty(), "剪切粘贴成功后源已不在原处，剪贴板必须失效");
    }

    #[test]
    fn a_partially_failed_cut_keeps_the_clipboard_for_a_retry() {
        let mut clipboard = Clipboard::new();
        clipboard.put(ClipboardMode::Cut, vec![path(r"C:\a\x.txt")], path(r"C:\a"));
        let plan = clipboard.plan(&path(r"C:\b")).unwrap();

        let report = OpReport {
            failed: vec![(path(r"C:\a\x.txt"), String::from("文件正被其它程序使用"))],
            ..OpReport::default()
        };
        clipboard.complete(&plan, &report);

        assert!(
            !clipboard.is_empty(),
            "一条都没成功时剪贴板必须留着，否则用户得回去重选一遍"
        );
    }

    #[test]
    fn cutting_in_place_is_rejected() {
        let mut clipboard = Clipboard::new();
        clipboard.put(ClipboardMode::Cut, vec![path(r"C:\a\x.txt")], path(r"C:\a"));

        assert_eq!(
            clipboard.plan(&path(r"C:\a")),
            Err(PasteError::SameLocation),
            "原位粘贴剪切内容只会得到一堆 x (2) 副本"
        );
        // 复制在原位粘贴是**合理**的（想要一份副本），不能一起拦掉。
        let mut clipboard = copied();
        assert!(clipboard.plan(&path(r"C:\a")).is_ok());
        clipboard.clear();
    }

    #[test]
    fn putting_an_empty_selection_clears_instead_of_leaving_a_stale_clipboard() {
        let mut clipboard = copied();
        clipboard.put(ClipboardMode::Copy, Vec::new(), path(r"C:\a"));
        assert!(
            clipboard.is_empty(),
            "空选中不能留下「有剪贴板但没条目」的状态，否则粘贴菜单项会莫名可用"
        );
    }

    #[test]
    fn error_messages_match_the_design_wording() {
        assert_eq!(PasteError::Empty.message(), "剪贴板为空");
        assert_eq!(PasteError::SameLocation.message(), "目标位置与源位置相同");
    }

    #[test]
    fn mode_wording_distinguishes_verb_from_action() {
        assert_eq!(ClipboardMode::Cut.verb(), "剪切");
        assert_eq!(ClipboardMode::Cut.action(), "移动");
        assert_eq!(ClipboardMode::Copy.verb(), "复制");
        assert_eq!(ClipboardMode::Copy.action(), "复制");
    }
}
