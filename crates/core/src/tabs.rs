//! 多标签页集合。

use crate::location::Location;
use crate::tab::{TabSnapshot, TabState};

/// 标签页的唯一标识。
///
/// 用 id 而不是下标来定位标签，是因为关闭标签会让后续下标整体前移：
/// 任何持有旧下标的异步回调或事件处理器都会因此操作到**错误的标签**。
/// 例如「点击标签上的关闭按钮」会被父级标签的点击处理器再次看到，
/// 此时若用下标定位，就会在已关闭的标签上继续操作。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TabId(pub u64);

/// 一个标签及其标识。
#[derive(Clone, Debug)]
pub struct Tab {
    id: TabId,
    state: TabState,
}

impl Tab {
    /// 标签标识。
    pub fn id(&self) -> TabId {
        self.id
    }

    /// 标签状态。
    pub fn state(&self) -> &TabState {
        &self.state
    }

    /// 标签状态（可变）。
    pub fn state_mut(&mut self) -> &mut TabState {
        &mut self.state
    }
}

/// 所有标签页及其激活状态。
///
/// 不变量：`tabs` 永不为空，`active` 始终落在合法下标上。
/// 关闭最后一个标签时不是把集合清空，而是把它重置为一个位于默认路径的新标签。
/// 这条不变量让「当前标签」的读取方可以无条件地直接取值。
#[derive(Clone, Debug)]
pub struct TabsState {
    tabs: Vec<Tab>,
    active: usize,
    next_id: u64,
    /// 关闭最后一个标签时重置到的位置，也用作新标签的初始位置。
    ///
    /// 存 `Location` 而不是 `PathBuf`：默认落点也可能是一个虚拟位置（例如「此电脑」）。
    default_location: Location,
    /// 已关闭标签的栈（最近关闭的在最后），供 Ctrl+Shift+T 恢复。
    ///
    /// 有界（见 [`MAX_CLOSED_TABS`]）：否则长时间运行的进程会为每个关闭过的
    /// 标签留一份状态，纯属浪费内存。
    closed: Vec<Tab>,
}

/// 「重新打开关闭的标签页」最多能回退多少步。
///
/// 16 是对齐浏览器标签恢复的惯例量级；超过这个深度时最早关闭的会被挤出栈。
const MAX_CLOSED_TABS: usize = 16;

impl TabsState {
    /// 创建只含一个标签的集合。
    pub fn new(default_location: impl Into<Location>) -> Self {
        let default_location = default_location.into();
        Self {
            tabs: vec![Tab {
                id: TabId(0),
                state: TabState::new(default_location.clone()),
            }],
            active: 0,
            next_id: 1,
            default_location,
            closed: Vec::new(),
        }
    }

    /// 由持久化快照整体还原。任何一份快照不合法就返回 `None`。
    ///
    /// 全有或全无：部分还原会得到「标签数量与历史对不上」的状态，
    /// 那种状态既不好解释，也会让 `Ctrl+Shift+T` 的关闭栈含义变得可疑。
    /// 调用方在 `None` 时退回「单个默认标签」即可。
    ///
    /// 关闭栈刻意**不还原**：它是本次会话内的操作记录，
    /// 跨重启复活一个「可以重新打开的标签」没有对应的用户动作。
    pub fn from_snapshots(snapshots: Vec<TabSnapshot>, active: usize) -> Option<Self> {
        if snapshots.is_empty() {
            return None;
        }
        let mut tabs = Vec::with_capacity(snapshots.len());
        for (index, snapshot) in snapshots.into_iter().enumerate() {
            let state = TabState::from_snapshot(snapshot)?;
            tabs.push(Tab {
                id: TabId(index as u64),
                state,
            });
        }
        // 越界的 active 收敛到最后一个标签，而不是拒绝整份会话：
        // 标签数量对得上而激活项越界，说明会话文件被外部改过，收敛即可用。
        let active = active.min(tabs.len() - 1);
        let default_location = tabs[0].state().location().clone();
        let next_id = tabs.len() as u64;
        Some(Self {
            tabs,
            active,
            next_id,
            default_location,
            closed: Vec::new(),
        })
    }

    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    /// 恒为 `false`（见类型级不变量），保留它只是为了满足 `len`/`is_empty` 的成对约定。
    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    pub fn active_index(&self) -> usize {
        self.active
    }

    pub fn active(&self) -> &Tab {
        &self.tabs[self.active]
    }

    pub fn active_state(&self) -> &TabState {
        self.active().state()
    }

    pub fn active_state_mut(&mut self) -> &mut TabState {
        let active = self.active;
        self.tabs[active].state_mut()
    }

    /// 按 id 查找下标。
    pub fn index_of(&self, id: TabId) -> Option<usize> {
        self.tabs.iter().position(|tab| tab.id == id)
    }

    /// 按 id 取标签状态。
    pub fn state_of(&self, id: TabId) -> Option<&TabState> {
        self.index_of(id).map(|index| self.tabs[index].state())
    }

    /// 按 id 取标签状态（可变）。
    pub fn state_of_mut(&mut self, id: TabId) -> Option<&mut TabState> {
        let index = self.index_of(id)?;
        Some(self.tabs[index].state_mut())
    }

    /// 选中标签；id 不存在时返回 `false`。
    pub fn select(&mut self, id: TabId) -> bool {
        match self.index_of(id) {
            Some(index) => {
                self.active = index;
                true
            }
            None => false,
        }
    }

    /// 新建标签（初始位置为默认落点）并选中它。
    pub fn open(&mut self) -> TabId {
        self.open_at(self.default_location.clone())
    }

    /// 新建标签并**沿用当前标签的视图模式**。
    ///
    /// `TabState::new` 的默认值是图标视图。用户切到详细信息视图后再开新标签，
    /// 若直接 `open_at`，新标签会被打回图标视图——看起来就像「设置没被记住」。
    /// 视图模式属于「用户的工作方式」而不是「某个目录的属性」，因此要沿袭。
    ///
    /// 放在 core 而不是外壳：这条规则只依赖 `TabsState` 自己的状态，
    /// 不涉及窗口，因此可以直接单测。
    pub fn open_at_like_active(&mut self, location: impl Into<Location>) -> TabId {
        let mode = self.active_state().view_mode();
        let id = self.open_at(location);
        self.active_state_mut().set_view_mode(mode);
        id
    }

    /// 新建标签并直接定位到 `location`，同时选中它。
    pub fn open_at(&mut self, location: impl Into<Location>) -> TabId {
        let id = TabId(self.next_id);
        self.next_id += 1;
        self.tabs.push(Tab {
            id,
            state: TabState::new(location),
        });
        self.active = self.tabs.len() - 1;
        id
    }

    /// 关闭标签，返回关闭后处于选中态的标签 id。
    ///
    /// 关闭当前标签时优先落到右侧邻居，没有右侧则落到左侧（与浏览器一致）。
    /// 关闭最后一个标签时重置为一个位于默认路径的新标签。
    /// id 不存在时不做任何改动，并返回当前激活标签的 id。
    pub fn close(&mut self, id: TabId) -> TabId {
        let Some(index) = self.index_of(id) else {
            return self.active().id();
        };

        // 先记下被关的标签，供 Ctrl+Shift+T 恢复。连「关闭最后一个标签触发的
        // 重置」也记：那一刻被替换掉的标签同样是「用户关掉的东西」。
        self.record_closed(self.tabs[index].clone());

        if self.tabs.len() == 1 {
            let fresh = TabId(self.next_id);
            self.next_id += 1;
            self.tabs = vec![Tab {
                id: fresh,
                state: TabState::new(self.default_location.clone()),
            }];
            self.active = 0;
            return fresh;
        }

        self.tabs.remove(index);
        if index < self.active {
            // 被关闭的标签在激活标签左侧：下标整体前移一位，激活的仍是同一个标签。
            self.active -= 1;
        } else if index == self.active {
            // 优先落到右侧邻居；已经是最后一个则落到左侧。
            self.active = index.min(self.tabs.len() - 1);
        }
        self.active().id()
    }

    /// 关闭当前标签。
    pub fn close_active(&mut self) -> TabId {
        let id = self.active().id();
        self.close(id)
    }

    /// 关闭除 `id` 之外的所有标签，`id` 成为唯一剩余并保持选中。
    ///
    /// `id` 不存在时不做任何改动。
    pub fn close_others(&mut self, id: TabId) {
        let Some(keep) = self.index_of(id) else {
            return;
        };
        // 从右往左删、跳过要保留的那一个：这样删除不会让未处理的下标前移错位。
        for index in (0..self.tabs.len()).rev() {
            if index == keep {
                continue;
            }
            let removed = self.tabs.remove(index);
            self.record_closed(removed);
        }
        // 删除后只剩保留的那一个，它必然落在下标 0。
        self.active = 0;
    }

    /// 把 `id` 标签移动到下标 `to` 的位置（拖拽排序）。
    ///
    /// 激活标签的 id 在移动前记下、移动后重查，因此无论被移的是不是激活标签，
    /// 激活状态都跟着它走、保持正确。
    pub fn move_tab(&mut self, id: TabId, to: usize) {
        let Some(from) = self.index_of(id) else {
            return;
        };
        let to = to.min(self.tabs.len() - 1);
        if from == to {
            return;
        }
        let active_id = self.active().id();
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        self.active = self.index_of(active_id).expect("激活标签不应在拖拽中消失");
    }

    /// 关闭 `id` 右侧的所有标签，`id` 及其左侧保持不变。
    ///
    /// 原激活标签若落在被删的右侧区间，则落到 `id` 上（此刻它是新的最右端）。
    pub fn close_right(&mut self, id: TabId) {
        let Some(index) = self.index_of(id) else {
            return;
        };
        // 反复删掉最右端，直到 `id` 成为最右端为止。
        while self.tabs.len() > index + 1 {
            let last = self.tabs.len() - 1;
            let removed = self.tabs.remove(last);
            self.record_closed(removed);
        }
        if self.active > index {
            self.active = index;
        }
    }

    /// 恢复最近关闭的标签并选中它；栈空时返回 `None`。
    ///
    /// 复用标签原来的 id：id 一旦分配就永不复用（`next_id` 只增不减），
    /// 此刻它不在 `tabs` 里，重新放回是安全的。
    pub fn reopen_closed(&mut self) -> Option<TabId> {
        let tab = self.closed.pop()?;
        let id = tab.id();
        self.tabs.push(tab);
        self.active = self.tabs.len() - 1;
        Some(id)
    }

    /// 记录一个刚被关闭的标签，并把栈裁剪到 [`MAX_CLOSED_TABS`]。
    fn record_closed(&mut self, tab: Tab) {
        self.closed.push(tab);
        if self.closed.len() > MAX_CLOSED_TABS {
            self.closed.remove(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ViewMode;
    use std::path::{Path, PathBuf};

    /// 断言当前标签停在给定真实目录上。
    ///
    /// 走 `directory()` 而不是直接比较位置：这本身就顺带验证了
    /// 「真实目录能取出路径、虚拟位置取不出」这条不变量。
    fn assert_dir(tabs: &TabsState, expected: &str) {
        assert_eq!(tabs.active_state().directory(), Some(Path::new(expected)));
    }

    #[test]
    fn a_new_tab_inherits_the_view_mode_of_the_current_one() {
        // 用户切到详细信息视图后开新标签，新标签必须还是详细信息视图——
        // 否则就是「设置没被记住」的那种观感。
        let mut tabs = TabsState::new(Location::Directory(PathBuf::from(r"C:\Users")));
        tabs.active_state_mut().set_view_mode(ViewMode::Details);

        tabs.open_at_like_active(Location::Directory(PathBuf::from(r"C:\Windows")));

        assert_eq!(tabs.active_state().view_mode(), ViewMode::Details);

        // 反向也要成立：从图标视图开新标签，得到的仍是图标视图。
        tabs.active_state_mut().set_view_mode(ViewMode::Icons);
        tabs.open_at_like_active(Location::Directory(PathBuf::from(r"C:\Temp")));
        assert_eq!(tabs.active_state().view_mode(), ViewMode::Icons);
    }

    #[test]
    fn new_state_has_a_single_active_tab() {
        let tabs = TabsState::new(r"C:\home");
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs.active_index(), 0);
        assert_dir(&tabs, r"C:\home");
    }

    #[test]
    fn opening_a_tab_selects_it_and_ids_stay_unique() {
        let mut tabs = TabsState::new(r"C:\home");
        let first = tabs.active().id();
        let second = tabs.open_at(r"C:\a");

        assert_ne!(first, second);
        assert_eq!(tabs.active().id(), second);
        assert_eq!(tabs.len(), 2);
    }

    #[test]
    fn closing_the_active_tab_prefers_the_right_neighbour() {
        let mut tabs = TabsState::new(r"C:\home");
        let a = tabs.open_at(r"C:\a");
        let b = tabs.open_at(r"C:\b");

        assert!(tabs.select(a));
        let now_active = tabs.close(a);

        assert_eq!(now_active, b, "应落到右侧邻居");
        assert_dir(&tabs, r"C:\b");
        assert_eq!(tabs.len(), 2);
    }

    #[test]
    fn closing_the_rightmost_active_tab_falls_back_to_the_left() {
        let mut tabs = TabsState::new(r"C:\home");
        let a = tabs.open_at(r"C:\a");
        let b = tabs.open_at(r"C:\b");

        let now_active = tabs.close(b);

        assert_eq!(now_active, a, "右侧没有邻居时应落到左侧");
        assert_dir(&tabs, r"C:\a");
    }

    #[test]
    fn closing_a_tab_left_of_active_keeps_the_same_tab_selected() {
        let mut tabs = TabsState::new(r"C:\home");
        let a = tabs.open_at(r"C:\a");
        let b = tabs.open_at(r"C:\b");

        assert!(tabs.select(b));
        let now_active = tabs.close(a);

        assert_eq!(now_active, b, "关掉左侧标签不应改变当前标签");
        assert_dir(&tabs, r"C:\b");
    }

    #[test]
    fn closing_the_last_remaining_tab_resets_to_the_default_path() {
        let mut tabs = TabsState::new(r"C:\home");
        let only = tabs.active().id();
        let fresh = tabs.close(only);

        assert_eq!(tabs.len(), 1);
        assert_ne!(fresh, only, "重置后的标签应当是一个全新标签");
        assert_dir(&tabs, r"C:\home");
    }

    #[test]
    fn operations_with_a_stale_id_are_ignored() {
        let mut tabs = TabsState::new(r"C:\home");
        let stale = tabs.open_at(r"C:\a");
        tabs.close(stale);

        // 模拟「关闭按钮的点击又被父级标签的处理器看到」这种时序。
        assert!(!tabs.select(stale));
        assert_dir(&tabs, r"C:\home");
        assert_eq!(tabs.len(), 1);
    }

    #[test]
    fn default_location_may_be_a_virtual_position() {
        let mut tabs = TabsState::new(Location::Computer);

        // 新标签沿用同一个默认落点。
        let opened = tabs.open();
        assert!(tabs.select(opened));
        assert_eq!(tabs.active_state().location(), &Location::Computer);
        assert_eq!(tabs.active_state().directory(), None);

        // 关闭最后一个标签后应重置回「此电脑」，而不是退化成一个空路径。
        tabs.close(opened);
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs.active_state().location(), &Location::Computer);
    }

    #[test]
    fn closing_an_unknown_id_is_a_no_op() {
        let mut tabs = TabsState::new(r"C:\home");
        let before = tabs.active().id();
        assert_eq!(tabs.close(TabId(9999)), before);
        assert_eq!(tabs.len(), 1);
    }

    #[test]
    fn closed_tabs_reopen_in_reverse_order() {
        let mut tabs = TabsState::new(r"C:\home");
        let a = tabs.open_at(r"C:\a");
        let b = tabs.open_at(r"C:\b");
        tabs.close(b);
        tabs.close(a);

        // 后关的先回来。
        let reopened = tabs.reopen_closed().expect("应当能恢复 a");
        assert_eq!(reopened, a);
        assert_eq!(tabs.active().id(), a);
        assert_eq!(tabs.len(), 2);

        let reopened = tabs.reopen_closed().expect("应当能恢复 b");
        assert_eq!(reopened, b);
        assert_eq!(tabs.len(), 3);
    }

    #[test]
    fn close_others_keeps_only_the_given_tab() {
        let mut tabs = TabsState::new(r"C:\home");
        let a = tabs.open_at(r"C:\a");
        tabs.open_at(r"C:\b");

        tabs.close_others(a);
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs.active().id(), a);
    }

    #[test]
    fn close_right_removes_only_tabs_to_the_right() {
        let mut tabs = TabsState::new(r"C:\home");
        let home = tabs.active().id();
        let a = tabs.open_at(r"C:\a");
        let b = tabs.open_at(r"C:\b");
        // 顺序：home, a, b；激活标签是 b。

        tabs.close_right(a);
        assert_eq!(tabs.len(), 2);
        assert_eq!(tabs.index_of(home), Some(0));
        assert_eq!(tabs.index_of(a), Some(1));
        assert_eq!(tabs.index_of(b), None, "b 在 a 右侧，应被关闭");
        assert_eq!(tabs.active().id(), a, "激活标签落到 a 上");
    }

    #[test]
    fn move_tab_reorders_and_keeps_active() {
        let mut tabs = TabsState::new(r"C:\home");
        let a = tabs.open_at(r"C:\a");
        let b = tabs.open_at(r"C:\b");
        // 顺序：home, a, b；active = b（下标 2）。

        tabs.move_tab(a, 0);
        assert_eq!(tabs.index_of(a), Some(0));
        assert_eq!(tabs.index_of(b), Some(2));
        assert_eq!(tabs.active().id(), b, "移动非激活标签不应改变激活");
    }

    #[test]
    fn move_active_tab_keeps_it_active() {
        let mut tabs = TabsState::new(r"C:\home");
        tabs.open_at(r"C:\a");
        let b = tabs.open_at(r"C:\b");
        // 把激活的 b（下标 2）移到最前。
        tabs.move_tab(b, 0);
        assert_eq!(tabs.index_of(b), Some(0));
        assert_eq!(tabs.active().id(), b);
    }

    #[test]
    fn snapshots_restore_order_history_and_active_tab() {
        let mut tabs = TabsState::new(r"C:\home");
        tabs.open_at(r"C:\a");
        let c = tabs.open_at(r"C:\b");
        assert!(tabs.select(c));

        let snapshots: Vec<crate::TabSnapshot> =
            tabs.tabs().iter().map(|tab| tab.state().snapshot()).collect();
        let active = tabs.active_index();

        let restored = TabsState::from_snapshots(snapshots, active).expect("应能还原会话");
        assert_eq!(restored.len(), 3);
        assert_eq!(restored.active_index(), 2);
        assert_tab_dir(&restored, 0, r"C:\home");
        assert_tab_dir(&restored, 1, r"C:\a");
        assert_tab_dir(&restored, 2, r"C:\b");
        // 还原出来的标签 id 必须互不相同，后续 open_at 也不能与之冲突。
        let ids: Vec<TabId> = restored.tabs().iter().map(|tab| tab.id()).collect();
        assert_eq!(ids.len(), 3);
        assert_ne!(ids[0], ids[1]);
        assert_ne!(ids[1], ids[2]);
    }

    #[test]
    fn restoring_rejects_empty_or_invalid_sessions() {
        assert!(TabsState::from_snapshots(Vec::new(), 0).is_none());

        // 一份坏快照会让整份会话被拒绝：宁可回到单标签，也不要半对半错的历史。
        let good = TabsState::new(r"C:\home").active_state().snapshot();
        let bad = crate::TabSnapshot {
            history: Vec::new(),
            cursor: 0,
            view_mode: crate::ViewMode::Details,
        };
        assert!(TabsState::from_snapshots(vec![good, bad], 0).is_none());
    }

    #[test]
    fn restoring_clamps_an_out_of_range_active_index() {
        let snapshot = TabsState::new(r"C:\home").active_state().snapshot();
        let restored = TabsState::from_snapshots(vec![snapshot], 7).expect("应能还原");
        assert_eq!(restored.active_index(), 0);
    }

    fn assert_tab_dir(tabs: &TabsState, index: usize, expected: &str) {
        assert_eq!(
            tabs.tabs()[index].state().directory(),
            Some(std::path::Path::new(expected))
        );
    }
}
