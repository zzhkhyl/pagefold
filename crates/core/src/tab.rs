//! 标签页状态：每个标签独立维护当前位置、前进后退历史、视图状态与扫描代数。

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::entry::{SortDirection, SortKey, ViewMode};
use crate::location::Location;

/// 一个标签页的**可持久化**快照。
///
/// 只包含「重启之后仍然有意义」的字段：历史、游标位置与视图模式。
/// 刻意不存扫描结果、选中集与滚动位置——它们要么能重新算出来，
/// 要么在下次启动时早已失效（目录内容可能已经变了）。
///
/// 类型的字段全部 `pub`：它是一份纯数据，校验规则集中在
/// [`TabState::from_snapshot`]，因此不需要额外的构造函数。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabSnapshot {
    /// 完整导航历史（含当前位置）。
    pub history: Vec<Location>,
    /// 当前项在历史中的下标。
    pub cursor: usize,
    /// 内容区视图模式。
    pub view_mode: ViewMode,
}

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

    /// 生成可持久化快照。
    ///
    /// 历史是私有的，因此快照只能从这里拿——否则 `services` 层就得复制一份
    /// 「历史怎么维护」的知识，迟早与这里分叉。
    pub fn snapshot(&self) -> TabSnapshot {
        TabSnapshot {
            history: self.history.clone(),
            cursor: self.cursor,
            view_mode: self.view_mode,
        }
    }

    /// 由快照还原。数据不合法（历史为空、游标越界）时返回 `None`。
    ///
    /// **不 panic、不部分接受**：快照来自磁盘，可能是被截断的文件或旧版本写下的内容。
    /// 还原失败时由调用方整体退回「单个默认标签」，而不是留下一个游标错位的标签——
    /// 后者会在渲染时直接下标越界。
    pub fn from_snapshot(snapshot: TabSnapshot) -> Option<Self> {
        if snapshot.history.is_empty() || snapshot.cursor >= snapshot.history.len() {
            return None;
        }
        Some(Self {
            history: snapshot.history,
            cursor: snapshot.cursor,
            view_mode: snapshot.view_mode,
            // 排序键与方向不持久化：目录内容可能变了，回到默认排序更可预期。
            sort_key: SortKey::default(),
            sort_direction: SortDirection::default(),
            // 代数从 0 开始即可：它是进程内的相对值，没有任何跨进程含义。
            generation: 0,
        })
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

    #[test]
    fn snapshot_round_trips_history_cursor_and_view_mode() {
        let mut tab = TabState::new(PathBuf::from(r"C:\a"));
        tab.navigate_to(PathBuf::from(r"C:\a\b"));
        tab.navigate_to(PathBuf::from(r"C:\a\b\c"));
        tab.back();
        tab.set_view_mode(ViewMode::Icons);

        let snapshot = tab.snapshot();
        assert_eq!(snapshot.cursor, 1, "游标必须指向当前位置");
        assert_eq!(snapshot.history.len(), 3);

        let restored = TabState::from_snapshot(snapshot).expect("合法快照应能还原");
        assert_eq!(restored.directory(), Some(Path::new(r"C:\a\b")));
        assert_eq!(restored.view_mode(), ViewMode::Icons);
        // 前进历史必须一起回来，否则「重启后 Alt+→」会失效。
        assert!(restored.can_go_forward());
        assert!(restored.can_go_back());
    }

    #[test]
    fn invalid_snapshots_are_rejected_instead_of_panicking() {
        // 空历史：渲染时 `history[cursor]` 会直接越界。
        assert!(TabState::from_snapshot(TabSnapshot {
            history: Vec::new(),
            cursor: 0,
            view_mode: ViewMode::Details,
        })
        .is_none());

        // 游标越界：同样会导致越界读取。
        assert!(TabState::from_snapshot(TabSnapshot {
            history: vec![PathBuf::from(r"C:\a").into()],
            cursor: 3,
            view_mode: ViewMode::Details,
        })
        .is_none());
    }

    #[test]
    fn snapshot_keeps_virtual_locations() {
        // 设置页/此电脑这类位置在历史里必须原样保留：还原成目录是错的。
        let mut tab = TabState::new(Location::Computer);
        tab.navigate_to(Location::Settings);
        let restored = TabState::from_snapshot(tab.snapshot()).expect("虚拟位置应可还原");
        assert_eq!(restored.location(), &Location::Settings);
        assert!(restored.can_go_back());
    }
}
