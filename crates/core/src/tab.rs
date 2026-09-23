//! 标签页状态：每个标签独立维护当前位置、前进后退历史、视图状态与扫描代数。

use std::path::Path;

use crate::entry::{SortDirection, SortKey, ViewMode};
use crate::location::Location;

/// 单个标签页的状态。
///
/// `generation` 是这套状态的核心：每次导航（含刷新）都会自增，
/// 在途的分批扫描结果回传时与之比对，不一致即整批丢弃。
/// 这样快速连续切换目录时既不会出现结果错乱，也不会产生无谓重绘。
#[derive(Clone, Debug)]
pub struct TabState {
    /// 导航历史，`cursor` 指向当前项；`cursor` 之后的都是可前进项。
    history: Vec<Location>,
    cursor: usize,
    view_mode: ViewMode,
    sort_key: SortKey,
    sort_direction: SortDirection,
    generation: u64,
}

impl TabState {
    /// 以 `location` 为初始位置创建标签页（历史长度为 1，前进后退均不可用）。
    pub fn new(location: impl Into<Location>) -> Self {
        Self {
            history: vec![location.into()],
            cursor: 0,
            view_mode: ViewMode::default(),
            sort_key: SortKey::default(),
            sort_direction: SortDirection::default(),
            generation: 0,
        }
    }

    /// 当前位置。
    pub fn location(&self) -> &Location {
        // 不变量：cursor 永远落在 history 的合法下标上，见下方所有写方法。
        &self.history[self.cursor]
    }

    /// 当前所在的真实目录；虚拟位置返回 `None`。
    ///
    /// 需要访问文件系统的调用点都必须走这里，因此不可能把「此电脑」误当目录去扫描。
    pub fn directory(&self) -> Option<&Path> {
        self.location().as_dir()
    }

    /// 导航到新位置，并返回新的扫描代数。
    ///
    /// 落点之后的前进历史会被截断（与浏览器行为一致）。
    /// 目标与当前位置相同时不写历史，只推进代数——等价于一次刷新。
    pub fn navigate_to(&mut self, location: impl Into<Location>) -> u64 {
        let location = location.into();
        if self.location() == &location {
            return self.bump_generation();
        }
        self.history.truncate(self.cursor + 1);
        self.history.push(location);
        self.cursor = self.history.len() - 1;
        self.bump_generation()
    }

    pub fn can_go_back(&self) -> bool {
        self.cursor > 0
    }

    pub fn can_go_forward(&self) -> bool {
        self.cursor + 1 < self.history.len()
    }

    /// 后退一步。已在历史起点时返回 `None`，调用方无需先判断 `can_go_back`。
    pub fn back(&mut self) -> Option<u64> {
        if !self.can_go_back() {
            return None;
        }
        self.cursor -= 1;
        Some(self.bump_generation())
    }

    /// 前进一步。已在历史终点时返回 `None`。
    pub fn forward(&mut self) -> Option<u64> {
        if !self.can_go_forward() {
            return None;
        }
        self.cursor += 1;
        Some(self.bump_generation())
    }

    /// 进入上级目录。
    ///
    /// 两种情况返回 `None`：当前是「此电脑」这类虚拟位置（没有上级），
    /// 以及已经处于文件系统根目录（避免原地打转）。
    pub fn parent(&mut self) -> Option<u64> {
        let parent = self.directory()?.parent()?.to_path_buf();
        // 形如 `C:` 的相对根，`parent()` 会给出空路径，此时同样视为已到顶。
        if parent.as_os_str().is_empty() {
            return None;
        }
        Some(self.navigate_to(parent))
    }

    /// 刷新当前位置：不改动历史，只推进代数，使在途的旧批次结果全部失效。
    pub fn refresh(&mut self) -> u64 {
        self.bump_generation()
    }

    /// 当前扫描代数。扫描结果回传时必须比对。
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// 标签标题。
    pub fn title(&self) -> String {
        self.location().title()
    }

    pub fn view_mode(&self) -> ViewMode {
        self.view_mode
    }

    pub fn set_view_mode(&mut self, mode: ViewMode) {
        self.view_mode = mode;
    }

    pub fn sort_key(&self) -> SortKey {
        self.sort_key
    }

    pub fn sort_direction(&self) -> SortDirection {
        self.sort_direction
    }

    /// 点击表头：同键翻转方向，异键切到该键并回到升序。
    pub fn toggle_sort(&mut self, key: SortKey) {
        if self.sort_key == key {
            self.sort_direction = self.sort_direction.flipped();
        } else {
            self.sort_key = key;
            self.sort_direction = SortDirection::Ascending;
        }
    }

    fn bump_generation(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.generation
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn new_tab_has_no_history_in_either_direction() {
        let tab = TabState::new(PathBuf::from(r"C:\Users"));
        assert_eq!(tab.directory(), Some(Path::new(r"C:\Users")));
        assert!(!tab.can_go_back());
        assert!(!tab.can_go_forward());
    }

    #[test]
    fn navigate_truncates_forward_history() {
        let mut tab = TabState::new(PathBuf::from(r"C:\a"));
        tab.navigate_to(PathBuf::from(r"C:\a\b"));
        tab.navigate_to(PathBuf::from(r"C:\a\b\c"));
        assert!(tab.back().is_some());
        assert_eq!(tab.directory(), Some(Path::new(r"C:\a\b")));
        assert!(tab.can_go_forward());

        tab.navigate_to(PathBuf::from(r"C:\a\x"));
        assert_eq!(tab.directory(), Some(Path::new(r"C:\a\x")));
        assert!(!tab.can_go_forward());
        assert!(tab.can_go_back());
    }

    #[test]
    fn every_navigation_bumps_generation() {
        let mut tab = TabState::new(PathBuf::from(r"C:\a"));
        let first = tab.generation();

        // 导航到同一位置等价于刷新，也必须推进代数，否则在途批次会被误判为有效。
        let same = tab.navigate_to(PathBuf::from(r"C:\a"));
        assert_eq!(same, first.wrapping_add(1));

        let next = tab.navigate_to(PathBuf::from(r"C:\a\b"));
        assert_ne!(next, same);

        let back = tab.back().expect("should be able to go back");
        assert_ne!(back, next);
        let forward = tab.forward().expect("should be able to go forward");
        assert_ne!(forward, back);
        assert_ne!(tab.refresh(), forward);
    }

    #[test]
    fn history_boundaries_return_none() {
        let mut tab = TabState::new(PathBuf::from(r"C:\a"));
        assert!(tab.back().is_none());
        assert!(tab.forward().is_none());
    }

    #[test]
    fn title_falls_back_to_the_full_path_at_the_root() {
        assert_eq!(TabState::new(PathBuf::from(r"C:\Users\abc")).title(), "abc");
        // 根目录没有 file_name，必须退回完整路径而不是空串。
        assert_eq!(TabState::new(PathBuf::from(r"C:\")).title(), r"C:\");
    }

    #[test]
    fn toggle_sort_flips_only_on_same_key() {
        let mut tab = TabState::new(PathBuf::from(r"C:\a"));
        assert_eq!(tab.sort_key(), SortKey::Name);
        assert_eq!(tab.sort_direction(), SortDirection::Ascending);

        tab.toggle_sort(SortKey::Name);
        assert_eq!(tab.sort_direction(), SortDirection::Descending);

        tab.toggle_sort(SortKey::Size);
        assert_eq!(tab.sort_key(), SortKey::Size);
        assert_eq!(tab.sort_direction(), SortDirection::Ascending);
    }

    #[test]
    fn computer_location_has_no_directory_and_no_parent() {
        let mut tab = TabState::new(Location::Computer);
        assert_eq!(tab.directory(), None, "虚拟位置不得暴露目录");
        assert!(tab.parent().is_none(), "「此电脑」没有上级");
        assert_eq!(tab.title(), "此电脑");
    }

    #[test]
    fn navigating_between_computer_and_directory_is_recorded() {
        let mut tab = TabState::new(Location::Computer);
        tab.navigate_to(PathBuf::from(r"C:\Users"));

        assert_eq!(tab.directory(), Some(Path::new(r"C:\Users")));
        assert!(tab.can_go_back(), "从「此电脑」进入目录后应能返回");
        assert!(tab.back().is_some());
        assert_eq!(tab.location(), &Location::Computer);
        assert_eq!(tab.directory(), None);

        // 从目录进入「此电脑」同样算一次真实导航。
        let mut tab = TabState::new(PathBuf::from(r"C:\Users"));
        let before = tab.generation();
        let after = tab.navigate_to(Location::Computer);
        assert_ne!(after, before);
        assert_eq!(tab.directory(), None);
    }
}
