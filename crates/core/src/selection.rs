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

    /// Shift 点击语义的区间选中：以锚点为起点，把区间内的下标全部选中。
    ///
    /// 锚点缺失或已越界时退化为单选——否则「先 Shift 再点」会因为找不到起点而毫无反应。
    /// 注意区间选择是**追加**而不是替换，连续 Shift 点击才能像资源管理器那样逐步扩选。
    pub fn select_range(&mut self, index: usize, len: usize) {
        let Some(anchor) = self.anchor else {
            self.select_single(index);
            return;
        };
        if anchor >= len {
            self.select_single(index);
            return;
        }

        let (start, end) = if anchor <= index {
            (anchor, index)
        } else {
            (index, anchor)
        };
        self.indices.extend(start..=end);
    }

    /// 全选前 `len` 个下标（Ctrl+A）。
    ///
    /// 锚点落到首项，这样紧接着的 Shift 点击仍能按「从第一项开始」的直觉扩选。
    pub fn select_all(&mut self, len: usize) {
        self.indices = (0..len).collect();
        self.anchor = (len > 0).then_some(0);
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

    #[test]
    fn select_range_covers_both_directions_from_the_anchor() {
        let mut selection = Selection::default();
        selection.select_single(3);
        selection.select_range(6, 10);
        assert_eq!(selection.iter().collect::<Vec<_>>(), vec![3, 4, 5, 6]);

        // 反向 Shift：从锚点往回扩选，同样必须命中区间内的全部下标。
        let mut selection = Selection::default();
        selection.select_single(6);
        selection.select_range(4, 10);
        assert_eq!(selection.iter().collect::<Vec<_>>(), vec![4, 5, 6]);
    }

    #[test]
    fn select_range_without_anchor_falls_back_to_single() {
        let mut selection = Selection::default();
        selection.select_range(2, 10);
        assert_eq!(selection.single(), Some(2));
    }

    #[test]
    fn select_range_with_stale_anchor_falls_back_to_single() {
        // 目录内容变短后锚点可能越界，此时不能对越界区间做扩选。
        let mut selection = Selection::default();
        selection.select_single(9);
        selection.select_range(1, 3);
        assert_eq!(selection.single(), Some(1));
    }

    #[test]
    fn select_all_covers_every_index_and_resets_the_anchor() {
        let mut selection = Selection::default();
        selection.select_all(4);
        assert_eq!(selection.len(), 4);
        assert_eq!(selection.anchor(), Some(0));

        // 空列表：既没有选中项，也不该留下一个悬空锚点。
        selection.select_all(0);
        assert!(selection.is_empty());
        assert_eq!(selection.anchor(), None);
    }
}
