//! 撤销栈：只记录**可逆**的那几种写操作。
//!
//! ## 为什么只记录这四种
//!
//! 撤销需要一条「原路返回」的路径：
//! - 重命名：改回去即可；
//! - 新建：删掉刚建的东西；
//! - 复制粘贴：删掉复制出来的那些；
//! - 剪切粘贴：把它们搬回原来的父目录。
//!
//! **移到回收站不在其中**：它的逆操作是「还原」，需要读 `$I` 元数据并写回原位置，
//! 那套逻辑属于回收站的写能力（与「清空回收站」一起做）。因此这里不记它，
//! 也就不存在「撤销了删除但只还原了一半」这种半吊子状态。
//!
//! 栈深有上限：撤销栈是**便利**而不是事务日志，五十步之后的操作再怎么回忆
//! 也没有意义，而无限增长会让一次会话的路径全部常驻内存。

use std::collections::VecDeque;
use std::path::PathBuf;

/// 撤销栈的默认深度。
pub const UNDO_LIMIT: usize = 50;

/// 一次可撤销的写操作。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UndoAction {
    /// 重命名：从 `from` 改成 `to`（两者都在同一父目录下）。
    Rename { from: PathBuf, to: PathBuf },
    /// 新建：撤销即删除 `path`。
    Create { path: PathBuf },
    /// 复制粘贴：撤销即删除这些新产生的条目。
    Copy { created: Vec<PathBuf> },
    /// 剪切粘贴：撤销即把 `(现在的位置, 原来的位置)` 搬回去。
    Move { moved: Vec<(PathBuf, PathBuf)> },
}

impl UndoAction {
    /// 动作名，供菜单与提示使用（「撤销重命名」）。
    pub fn label(&self) -> &'static str {
        match self {
            Self::Rename { .. } => "重命名",
            Self::Create { .. } => "新建",
            Self::Copy { .. } => "复制",
            Self::Move { .. } => "移动",
        }
    }

    /// 影响到的条目数。
    pub fn count(&self) -> usize {
        match self {
            Self::Rename { .. } | Self::Create { .. } => 1,
            Self::Copy { created } => created.len(),
            Self::Move { moved } => moved.len(),
        }
    }

    /// 一句人话：「重命名「报告.docx」」/「复制 3 项」。
    ///
    /// 单项时尽量给出名字——撤销是不可预演的，用户需要在下手前确认
    /// 「我要撤的是不是刚才那一步」。
    pub fn describe(&self) -> String {
        match self {
            Self::Rename { to, .. } => {
                let name = to
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| to.display().to_string());
                format!("重命名「{name}」")
            }
            Self::Create { path } => {
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                format!("新建「{name}」")
            }
            Self::Copy { .. } | Self::Move { .. } => format!("{} {} 项", self.label(), self.count()),
        }
    }
}

/// 后进先出的撤销栈。
#[derive(Debug, Default)]
pub struct UndoStack {
    entries: VecDeque<UndoAction>,
}

impl UndoStack {
    pub fn new() -> Self {
        Self::default()
    }

    /// 压入一步操作。超过 [`UNDO_LIMIT`] 时丢弃最旧的一步。
    pub fn push(&mut self, action: UndoAction) {
        self.entries.push_back(action);
        while self.entries.len() > UNDO_LIMIT {
            self.entries.pop_front();
        }
    }

    /// 取出最近一步（`Ctrl+Z` 消费的那一步）。
    pub fn pop(&mut self) -> Option<UndoAction> {
        self.entries.pop_back()
    }

    /// 看一眼最近一步，不消费。菜单用它显示「撤销 重命名「x」」。
    pub fn peek(&self) -> Option<&UndoAction> {
        self.entries.back()
    }

    /// 清空。切换目录时**不**清——撤销栈是跨目录的：
    /// 用户「在这个目录剪切、去另一个目录粘贴、再按 Ctrl+Z」是正常用法。
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rename(from: &str, to: &str) -> UndoAction {
        UndoAction::Rename {
            from: PathBuf::from(from),
            to: PathBuf::from(to),
        }
    }

    #[test]
    fn pop_returns_the_most_recent_action() {
        let mut stack = UndoStack::new();
        stack.push(rename(r"C:\a.txt", r"C:\b.txt"));
        stack.push(UndoAction::Create {
            path: PathBuf::from(r"C:\新建文件夹"),
        });

        assert_eq!(stack.peek().map(UndoAction::label), Some("新建"));
        assert_eq!(stack.pop().map(|a| a.label()), Some("新建"));
        assert_eq!(stack.pop().map(|a| a.label()), Some("重命名"));
        assert!(stack.pop().is_none());
    }

    #[test]
    fn the_stack_drops_the_oldest_entry_at_the_limit() {
        let mut stack = UndoStack::new();
        for index in 0..(UNDO_LIMIT + 5) {
            stack.push(UndoAction::Create {
                path: PathBuf::from(format!(r"C:\item-{index}")),
            });
        }
        assert_eq!(stack.len(), UNDO_LIMIT);

        // 最旧的五步已经被丢弃，栈底应当是 `item-5`。
        let mut last = None;
        while let Some(action) = stack.pop() {
            last = Some(action);
        }
        assert_eq!(
            last,
            Some(UndoAction::Create {
                path: PathBuf::from(r"C:\item-5")
            })
        );
    }

    #[test]
    fn descriptions_name_the_single_item() {
        assert_eq!(
            rename(r"C:\a\报告.docx", r"C:\a\报告 (2).docx").describe(),
            "重命名「报告 (2).docx」"
        );
        assert_eq!(
            UndoAction::Create {
                path: PathBuf::from(r"C:\新建文件夹")
            }
            .describe(),
            "新建「新建文件夹」"
        );
    }

    #[test]
    fn batch_descriptions_report_the_count() {
        let action = UndoAction::Copy {
            created: vec![PathBuf::from(r"C:\a"), PathBuf::from(r"C:\b")],
        };
        assert_eq!(action.count(), 2);
        assert_eq!(action.describe(), "复制 2 项");
    }

    #[test]
    fn clearing_empties_the_stack() {
        let mut stack = UndoStack::new();
        stack.push(rename(r"C:\a", r"C:\b"));
        stack.clear();
        assert!(stack.is_empty());
        assert!(stack.pop().is_none());
    }
}
