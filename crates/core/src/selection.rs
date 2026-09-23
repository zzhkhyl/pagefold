//! 选择模型。
//!
//! 本次交付只需要单选（点击条目高亮）。框选、Ctrl/Shift 多选属后续阶段，
//! 因此底层存储直接使用有序集合、并额外维护 `anchor` 锚点，
//! 后续扩展为区间选择时不需要改动调用方。

use std::collections::BTreeSet;

/// 以「条目在视图模型中的下标」为标识的选中集合。
///
/// 用下标而非路径，是为了让选中态与列表的虚拟化渲染直接对齐（O(log n) 查找）。
#[derive(Clone, Debug, Default)]
pub struct Selection {
    indices: BTreeSet<usize>,
    /// 区间选择的锚点。单选路径下始终与唯一选中项一致，为后续 Shift 多选预留。
    anchor: Option<usize>,
}

impl Selection {
    pub fn clear(&mut self) {
        self.indices.clear();
        self.anchor = None;
    }

    /// 单选：清空后选中 `index`。
    pub fn select_single(&mut self, index: usize) {
        self.indices.clear();
        self.indices.insert(index);
        self.anchor = Some(index);
    }

    /// Ctrl 点击语义的切换选中。
    pub fn toggle(&mut self, index: usize) {
        if !self.indices.remove(&index) {
            self.indices.insert(index);
        }
        self.anchor = Some(index);
    }

    pub fn contains(&self, index: usize) -> bool {
        self.indices.contains(&index)
    }

    pub fn len(&self) -> usize {
        self.indices.len()
    }

    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.indices.iter().copied()
    }

    /// 单选时的唯一选中项；多选或空选返回 `None`。
    pub fn single(&self) -> Option<usize> {
        if self.indices.len() == 1 {
            self.indices.iter().next().copied()
        } else {
            None
        }
    }

    /// 区间选择锚点。
    pub fn anchor(&self) -> Option<usize> {
        self.anchor
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_single_replaces_previous_selection() {
        let mut selection = Selection::default();
        selection.select_single(3);
        selection.select_single(7);

        assert_eq!(selection.len(), 1);
        assert!(!selection.contains(3));
        assert_eq!(selection.single(), Some(7));
    }

    #[test]
    fn toggle_adds_and_removes() {
        let mut selection = Selection::default();
        selection.toggle(1);
        selection.toggle(2);
        assert_eq!(selection.len(), 2);
        // 多选状态下 single() 必须返回 None，避免调用方误当作单选处理。
        assert_eq!(selection.single(), None);

        selection.toggle(1);
        assert_eq!(selection.single(), Some(2));
    }
}
