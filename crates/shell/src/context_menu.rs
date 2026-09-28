//! 右键上下文菜单的**内容**。
//!
//! 刻意拆成两层：
//!
//! 1. [`entry_menu_spec`] / [`area_menu_spec`] 是**纯函数**——只吃 `fm_core` 的数据，
//!    吐出「有哪些项、哪项禁用、哪项打勾、怎么分组」的描述。菜单的动态性
//!    （目录与文件不同、驱动器少几项、多选改文案、当前排序键打勾）全部收敛在这一层，
//!    因此可以用单测锁死，不必起窗口去点。
//! 2. [`build_menu`] 只做搬运：把描述逐节点翻译成 `PopupMenu`，不含任何判断。
//!
//! 弹出层的定位、边缘吸附、`Esc` 与点击外部关闭、上下键切换、回车确认
//! 由 `gpui_component::menu::PopupMenu` 提供；挂载点见 `fm_views::context_menu`。
//! 配角（菜单底面、悬停底色、圆角）来自组件库主题，而
//! [`crate::shell::apply_component_theme`] 已把它们同步到设计令牌，故深浅色自动跟随。
//!
//! # 与设计稿的两处刻意差异
//!
//! 设计稿的 `.ctx-item` 还定义了两样东西，本版本没有照搬：
//!
//! - **快捷键提示**（`kbd`：Ctrl+C / F2 / Delete / F5 / Ctrl+V）。
//!   `crates/app/src/actions.rs` 目前是空壳，全局键位一个都没注册
//!   （README「已知缺口」也记着「目前所有操作都需要鼠标」）。照搬这些提示
//!   等于向用户承诺一组按下去毫无反应的快捷键，比不显示更糟。
//!   待键位接入后，把 hint 加回 [`MenuItemSpec`] 即可。
//! - **危险项配色**（`.is-danger` 的红色）。组件库的菜单项渲染没有危险态变体，
//!   而为了这一项改成自绘会连带丢掉「图标列对齐、勾选态、禁用态」等现成能力，
//!   不划算。「移到回收站」因此在菜单里与普通项同色，但它**默认禁用**，
//!   不会产生「点了就会删」的误导。

use std::path::{Path, PathBuf};
use std::rc::Rc;

use fm_core::{Entry, EntryKind, SortKey, ViewMode};
use gpui::{App, ClickEvent, Context, WeakEntity, Window};
use gpui_component::Icon;
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit_assets::IconName;

use crate::ShellView;

/// 菜单项被点击后要做的事。
///
/// 做成枚举而不是直接存闭包，是为了让描述层保持可比较、可断言——
/// 单测能直接比对「目录该有『在新标签页中打开』、驱动器不该有」。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum MenuAction {
    /// 打开：目录进入，文件不处理（与双击完全同一语义）。
    Open,
    /// 在指定目录开一个新标签并切过去。
    OpenInNewTab(PathBuf),
    /// 重新扫描当前目录。
    Refresh,
    /// 显示右侧预览面板（设计稿的「属性」就是它，不是独立对话框）。
    ShowPreview,
    /// 切换内容区视图模式。
    SetViewMode(ViewMode),
    /// 按指定键排序。再次点同一键会翻转方向，由 `TabState::toggle_sort` 负责。
    SetSort(SortKey),
    /// 文件写操作的占位：菜单里保留位置，但**禁用**。
    NotImplemented,
}

/// 一个菜单节点。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum MenuNode {
    Item(MenuItemSpec),
    /// 分组分隔线。
    Separator,
    /// 子菜单。`items` 里同样可以再放分隔线与子菜单。
    Submenu {
        label: String,
        icon: IconName,
        items: Vec<MenuNode>,
    },
}

/// 一个可点击 / 可禁用的菜单项。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MenuItemSpec {
    pub label: String,
    pub icon: IconName,
    /// 禁用（设计稿 `.ctx-item[disabled]{opacity:.4}`）。
    pub disabled: bool,
    /// 当前生效的项打勾（排序列、视图模式）。
    pub checked: bool,
    pub action: MenuAction,
}

impl MenuItemSpec {
    fn new(label: impl Into<String>, icon: IconName, action: MenuAction) -> Self {
        Self {
            label: label.into(),
            icon,
            disabled: false,
            checked: false,
            action,
        }
    }

    /// 尚未接入的文件写操作。
    ///
    /// 与命令条的「新建」按钮、选中条的动作组用同一套约定：**保留设计稿里的位置**
    /// 但置为禁用。直接删掉这一项会让菜单看起来「本来就没有这个功能」，
    /// 而写操作是文件管理器的核心动作，用户需要知道它即将到来。
    fn not_implemented(label: impl Into<String>, icon: IconName) -> Self {
        Self {
            label: label.into(),
            icon,
            disabled: true,
            checked: false,
            action: MenuAction::NotImplemented,
        }
    }

    fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }
}

/// 条目右键菜单的节点清单。
///
/// 逐项对齐 `design/file-manager.html` 里 `#files` 的 `oncontextmenu`：
/// 打开 / 在新标签页中打开 /（分隔）/ 复制 / 剪切 / 重命名 /（分隔）/ 属性 / 移到回收站。
/// 其中复制、剪切、重命名、移到回收站属于**文件写操作**，README「已知缺口」记载
/// 尚未接入，因此保留位置但置为禁用。
///
/// `selected` 是当前选中项数量，用于多选文案（设计稿的「复制 3 项」）。
/// `current_dir` 是当前所在目录，供「在新标签页中打开」决定落点：
/// 目录落在自身路径，文件落在当前目录——与设计稿 `addTab(...)` 的分支一致。
pub(crate) fn entry_menu_spec(
    entry: &Entry,
    selected: usize,
    current_dir: Option<&Path>,
) -> Vec<MenuNode> {
    // 驱动器是卷而不是普通条目：改名、进回收站、以及「在它的子目录开新标签」
    // 这几件事在语义上都不成立，见下面的逐项说明。
    let is_drive = matches!(entry.kind, EntryKind::Drive);
    let multi = selected > 1;
    // 设计稿的写法是 `复制${n>1?' '+n+' 项':''}`：只有多选时才带数量与「项」。
    let count = if multi {
        format!(" {selected} 项")
    } else {
        String::new()
    };

    let mut nodes = vec![MenuNode::Item(MenuItemSpec::new(
        "打开",
        IconName::FolderOpen,
        MenuAction::Open,
    ))];

    let new_tab_target = if entry.is_dir() {
        Some(entry.path.as_ref().clone())
    } else {
        current_dir.map(Path::to_path_buf)
    };
    if let Some(target) = new_tab_target {
        nodes.push(MenuNode::Item(MenuItemSpec::new(
            "在新标签页中打开",
            IconName::Plus,
            MenuAction::OpenInNewTab(target),
        )));
    }

    nodes.push(MenuNode::Separator);

    nodes.push(MenuNode::Item(MenuItemSpec::not_implemented(
        format!("复制{count}"),
        IconName::Copy,
    )));
    nodes.push(MenuNode::Item(MenuItemSpec::not_implemented(
        format!("剪切{count}"),
        IconName::Scissors,
    )));

    // 驱动器**不显示**改名与删除，而不是显示成禁用：禁用意味着「以后会有」，
    // 而「给整个磁盘改名」「把磁盘删进回收站」这两件事永远不会被实现。
    if !is_drive {
        nodes.push(MenuNode::Item(MenuItemSpec::not_implemented(
            if multi { "批量重命名" } else { "重命名" },
            IconName::Pencil,
        )));
    }

    nodes.push(MenuNode::Separator);

    // 设计稿的「属性」不是独立的属性对话框，而是把右侧预览面板打开。
    nodes.push(MenuNode::Item(MenuItemSpec::new(
        "属性",
        IconName::Info,
        MenuAction::ShowPreview,
    )));

    if !is_drive {
        nodes.push(MenuNode::Item(MenuItemSpec::not_implemented(
            "移到回收站",
            IconName::Trash,
        )));
    }

    nodes
}

/// 回收站里条目的右键菜单。
///
/// 只有「看」是有意义的。复制 / 剪切 / 重命名 / 移到回收站对回收站里的条目
/// 全都谈不是；真正该有的是**还原**与**彻底删除**，而那是写操作，
/// 不在本次范围内（方案 B 只做只读浏览）。
///
/// 因此这里**只给「属性」**，而不是把一排永远禁用的项摆上去：
/// 禁用意味着「以后会有」，而这两项并不在计划内。
pub(crate) fn recycle_bin_menu_spec() -> Vec<MenuNode> {
    vec![MenuNode::Item(MenuItemSpec::new(
        "属性",
        IconName::Info,
        MenuAction::ShowPreview,
    ))]
}

/// 内容区空白处右键菜单的节点清单。
///
/// 对齐设计稿 `#files` 右键处理里「没有命中行」的那条分支。
///
/// 设计稿末尾那段「回收站 / 清空回收站」仍没有对应实现：回收站这个虚拟位置
/// 现在已经有了（侧边栏可进入、只读浏览），但**清空回收站是写操作**，不在本次范围。
///
/// 视图模式两项的措辞取自命令条的 tooltip（「详细信息视图」/「图标视图」），
/// 而不是设计稿空白菜单里的「列表视图 / 网格视图」：同一个动作在应用内应该只有一个名字。
pub(crate) fn area_menu_spec(view_mode: ViewMode, sort_key: SortKey) -> Vec<MenuNode> {
    let view_node = |label: &str, icon: IconName, mode: ViewMode| {
        MenuNode::Item(
            MenuItemSpec::new(label, icon, MenuAction::SetViewMode(mode)).checked(view_mode == mode),
        )
    };
    let sort_node = |label: &str, icon: IconName, key: SortKey| {
        MenuNode::Item(MenuItemSpec::new(label, icon, MenuAction::SetSort(key)).checked(sort_key == key))
    };

    vec![
        MenuNode::Item(MenuItemSpec::not_implemented(
            "新建文件夹",
            IconName::Folder,
        )),
        MenuNode::Item(MenuItemSpec::not_implemented(
            "新建文本文档",
            IconName::FileText,
        )),
        MenuNode::Separator,
        MenuNode::Item(MenuItemSpec::not_implemented(
            "粘贴",
            IconName::ClipboardPaste,
        )),
        MenuNode::Item(MenuItemSpec::new(
            "刷新",
            IconName::RefreshCw,
            MenuAction::Refresh,
        )),
        MenuNode::Separator,
        // 排序收进子菜单：设计稿把它放在命令条的按钮上，这里用子菜单承载同一组动作，
        // 好处是主菜单不必变长，且当前排序键能用勾选态直接标出来。
        MenuNode::Submenu {
            label: String::from("排序方式"),
            icon: IconName::SortAscending,
            items: vec![
                sort_node("按名称", IconName::FileText, SortKey::Name),
                sort_node("按修改日期", IconName::Clock, SortKey::Modified),
                sort_node("按类型", IconName::Folder, SortKey::Kind),
                sort_node("按大小", IconName::List, SortKey::Size),
            ],
        },
        MenuNode::Separator,
        view_node("详细信息视图", IconName::List, ViewMode::Details),
        view_node("图标视图", IconName::LayoutGrid, ViewMode::Icons),
    ]
}

/// 把节点清单翻译成可弹出的菜单。
///
/// `entry` 为 `None` 表示空白处菜单——此时所有回调都不携带目标条目。
fn build_menu(
    mut menu: PopupMenu,
    nodes: &[MenuNode],
    entry: Option<&Entry>,
    shell: &WeakEntity<ShellView>,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    for node in nodes {
        menu = match node {
            MenuNode::Separator => menu.separator(),
            MenuNode::Item(spec) => {
                let mut item = PopupMenuItem::new(spec.label.clone())
                    .icon(Icon::new(spec.icon))
                    .disabled(spec.disabled)
                    .checked(spec.checked);
                // 禁用项不挂回调：组件库虽然也会拦住禁用项的点击，
                // 但少挂一个闭包就不会有「被绕过的回调」这种可能性。
                if !spec.disabled {
                    item = item.on_click(click_handler(shell, spec.action.clone(), entry.cloned()));
                }
                menu.item(item)
            }
            MenuNode::Submenu { label, icon, items } => {
                let child_shell = shell.clone();
                let child_entry = entry.cloned();
                let child_nodes = items.clone();
                menu.submenu_with_icon(
                    Some(Icon::new(*icon)),
                    label.clone(),
                    window,
                    cx,
                    move |submenu, window, cx| {
                        build_menu(
                            submenu,
                            &child_nodes,
                            child_entry.as_ref(),
                            &child_shell,
                            window,
                            cx,
                        )
                    },
                )
            }
        };
    }

    menu
}

/// 菜单项的回调。
///
/// 目标条目（以及它在 `visible()` 里的下标）在**构造菜单时**就被捕获进闭包，
/// 因此回调天然携带目标数据，不需要「当前右键目标」这种全局状态，
/// 也就不会有「菜单还开着、目标已经被切走」的竞态。
fn click_handler(
    shell: &WeakEntity<ShellView>,
    action: MenuAction,
    entry: Option<Entry>,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    let shell = shell.clone();
    move |_event: &ClickEvent, _window: &mut Window, cx: &mut App| {
        dispatch(&shell, &action, entry.as_ref(), cx);
    }
}

/// 执行一个菜单动作。
///
/// 一律经 `WeakEntity::update` 回到 `ShellView`，与 `open_handler` / `select_handler` 一致：
/// 菜单的生命周期可能比视图长（视图被销毁时菜单仍可能显示在屏幕上），不能持强引用。
fn dispatch(
    shell: &WeakEntity<ShellView>,
    action: &MenuAction,
    entry: Option<&Entry>,
    cx: &mut App,
) {
    let applied = shell.update(cx, |view, cx| match action {
        MenuAction::Open => {
            // 与双击同一语义：只有目录会被导航进入。文件要用默认程序打开，
            // 那需要 ShellExecute（属 fm-platform 的后续工作）；
            // 在那之前绝不能把文件路径交给 navigate_to——那会去扫描一个文件路径。
            if let Some(entry) = entry
                && entry.is_dir()
            {
                view.navigate_to(entry.path.as_ref().clone(), cx);
            }
        }
        MenuAction::OpenInNewTab(path) => view.open_tab_at(path.clone(), cx),
        MenuAction::Refresh => view.reload(cx),
        MenuAction::ShowPreview => view.show_preview(cx),
        MenuAction::SetViewMode(mode) => view.set_view_mode(*mode, cx),
        MenuAction::SetSort(key) => view.set_sort(*key, cx),
        MenuAction::NotImplemented => {}
    });

    if applied.is_err() {
        tracing::debug!("执行右键菜单项时视图已销毁，本次操作被忽略");
    }
}

/// 构造条目右键菜单的内容构造器，供内容区两处视图使用。
///
/// 选中数量与当前目录在**菜单弹出时**才读取，而不是在渲染时捕获：
/// 右键会先把目标项选中（见 [`right_press_handler`]），若沿用上一帧捕获的值，
/// 「复制 3 项」这类文案就会与用户眼前的选中状态差一拍。
pub(crate) fn entry_menu_builder(cx: &Context<ShellView>) -> fm_views::EntryMenuBuilder {
    let shell = cx.entity().downgrade();
    Rc::new(move |menu, entry, _index, window, cx| {
        let Ok((selected, current_dir, in_recycle_bin)) =
            shell.read_with(&**cx, |view: &ShellView, _| {
                let current_dir = view
                    .tabs
                    .active_state()
                    .location()
                    .as_dir()
                    .map(Path::to_path_buf);
                (view.selection.len(), current_dir, view.in_recycle_bin())
            })
        else {
            return menu;
        };

        // 回收站里的条目走另一份清单：导航类的项在那里没有意义，见
        // [`recycle_bin_menu_spec`]。
        let nodes = if in_recycle_bin {
            recycle_bin_menu_spec()
        } else {
            entry_menu_spec(entry, selected, current_dir.as_deref())
        };
        build_menu(menu, &nodes, Some(entry), &shell, window, cx)
    })
}

/// 构造内容区空白处的菜单构造器。
pub(crate) fn area_menu_builder(cx: &Context<ShellView>) -> fm_views::AreaMenuBuilder {
    let shell = cx.entity().downgrade();
    Rc::new(move |menu, window, cx| {
        let Ok((view_mode, sort_key)) = shell.read_with(&**cx, |view: &ShellView, _| {
            let tab = view.tabs.active_state();
            (tab.view_mode(), tab.sort_key())
        }) else {
            return menu;
        };

        let nodes = area_menu_spec(view_mode, sort_key);
        build_menu(menu, &nodes, None, &shell, window, cx)
    })
}

/// 右键按下时的处理器。
///
/// 与设计稿一致：右键落在**未选中**的项上时先把它选中。否则多选状态会被破坏——
/// 用户选中三项后右键其中一项，看到的是「复制 1 项」，与屏幕上的选中状态对不上。
/// 右键落在已选中的项上时保持多选，只把焦点项移过去。
pub(crate) fn right_press_handler(cx: &Context<ShellView>) -> fm_views::RightPressHandler {
    let view = cx.entity().downgrade();
    Rc::new(move |index: usize, _window: &mut Window, cx: &mut App| {
        let applied = view.update(cx, |view, cx| {
            if view.selection.contains(index) {
                if view.cursor != Some(index) {
                    view.cursor = Some(index);
                    cx.notify();
                }
            } else {
                view.select(index, fm_views::SelectMode::Replace, cx);
            }
        });

        if applied.is_err() {
            tracing::debug!("右键选中条目时视图已销毁，本次操作被忽略");
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::SystemTime;

    fn entry(name: &str, kind: EntryKind) -> Entry {
        Entry {
            name: Arc::from(name),
            path: Arc::new(PathBuf::from(format!("C:\\dir\\{name}"))),
            kind,
            size: 0,
            modified: Some(SystemTime::UNIX_EPOCH),
            icon: fm_core::IconId(0),
        }
    }

    fn directory(name: &str) -> Entry {
        entry(name, EntryKind::Directory)
    }

    fn file(name: &str) -> Entry {
        entry(name, EntryKind::File { ext: None })
    }

    fn drive(name: &str) -> Entry {
        entry(name, EntryKind::Drive)
    }

    /// 收集清单里所有菜单项的标签与禁用态，便于按名字断言。
    fn labels(nodes: &[MenuNode]) -> Vec<(String, bool)> {
        nodes
            .iter()
            .filter_map(|node| match node {
                MenuNode::Item(spec) => Some((spec.label.clone(), spec.disabled)),
                _ => None,
            })
            .collect()
    }

    fn find<'a>(nodes: &'a [MenuNode], label: &str) -> Option<&'a MenuItemSpec> {
        nodes.iter().find_map(|node| match node {
            MenuNode::Item(spec) if spec.label == label => Some(spec),
            _ => None,
        })
    }

    /// 所有写操作都必须禁用——它们是 README 记载「尚未接入」的那批动作。
    fn assert_write_ops_disabled(nodes: &[MenuNode]) {
        for label in ["复制", "剪切", "重命名", "移到回收站"] {
            if let Some(spec) = find(nodes, label) {
                assert!(spec.disabled, "{label} 属于文件写操作，必须是禁用态");
                assert_eq!(spec.action, MenuAction::NotImplemented);
            }
        }
    }

    #[test]
    fn directory_menu_opens_in_new_tab_at_its_own_path() {
        let dir = directory("子目录");
        let nodes = entry_menu_spec(&dir, 1, Some(Path::new("C:\\dir")));

        let spec = find(&nodes, "在新标签页中打开").expect("目录应当有「在新标签页中打开」");
        assert_eq!(
            spec.action,
            MenuAction::OpenInNewTab(PathBuf::from("C:\\dir\\子目录")),
            "目录的新标签应落在它自己身上，而不是当前目录"
        );
        assert_write_ops_disabled(&nodes);
    }

    #[test]
    fn file_menu_opens_in_new_tab_at_the_current_directory() {
        let file = file("报告.docx");
        let nodes = entry_menu_spec(&file, 1, Some(Path::new("C:\\dir")));

        let spec = find(&nodes, "在新标签页中打开").expect("文件也应当有这一项");
        assert_eq!(
            spec.action,
            MenuAction::OpenInNewTab(PathBuf::from("C:\\dir")),
            "文件没有自己的「目录」，新标签应落在当前目录"
        );
    }

    #[test]
    fn file_without_a_current_directory_hides_the_new_tab_item() {
        // 「此电脑」不是真实目录，取不到落点时这一项必须**整项消失**，
        // 而不是留一个点了没反应的入口。
        let file = file("报告.docx");
        let nodes = entry_menu_spec(&file, 1, None);
        assert!(find(&nodes, "在新标签页中打开").is_none());
    }

    #[test]
    fn drive_menu_hides_operations_that_make_no_sense_for_a_volume() {
        let nodes = entry_menu_spec(&drive("D:"), 1, None);

        // 隐藏（而不是禁用）：这三件事对「卷」永远不会有实现。
        assert!(find(&nodes, "重命名").is_none(), "驱动器不该出现改名");
        assert!(find(&nodes, "移到回收站").is_none(), "驱动器不该进回收站");
        assert!(find(&nodes, "在新标签页中打开").is_none());

        // 仍然保留了「打开」与「属性」。
        assert!(find(&nodes, "打开").is_some());
        assert!(find(&nodes, "属性").is_some());
    }

    #[test]
    fn multi_selection_changes_the_wording() {
        let dir = directory("子目录");
        let single = entry_menu_spec(&dir, 1, None);
        assert_eq!(find(&single, "复制").map(|s| s.label.clone()), Some(String::from("复制")));
        assert!(find(&single, "重命名").is_some());

        let many = entry_menu_spec(&dir, 3, None);
        assert!(find(&many, "复制 3 项").is_some(), "多选时文案要带上数量");
        assert!(find(&many, "剪切 3 项").is_some());
        assert!(find(&many, "批量重命名").is_some());
        assert!(find(&many, "重命名").is_none(), "多选不该再显示单项重命名");
    }

    #[test]
    fn recycle_bin_menu_offers_only_what_actually_works() {
        let nodes = recycle_bin_menu_spec();

        // 只有一项，而且是可用的「属性」——不是一排禁用项。
        assert_eq!(nodes.len(), 1);
        let spec = find(&nodes, "属性").expect("应当有「属性」");
        assert!(!spec.disabled, "「属性」（显示预览面板）是已实现能力");
        assert_eq!(spec.action, MenuAction::ShowPreview);

        // 明确不该出现的几项：它们对回收站里的条目没有意义。
        for label in ["打开", "在新标签页中打开", "重命名", "移到回收站"] {
            assert!(find(&nodes, label).is_none(), "{label} 不该出现在回收站菜单里");
        }
    }

    #[test]
    fn area_menu_marks_the_current_view_mode_and_sort_key() {
        let nodes = area_menu_spec(ViewMode::Icons, SortKey::Size);

        let icons = find(&nodes, "图标视图").expect("应当有图标视图");
        let details = find(&nodes, "详细信息视图").expect("应当有详细信息视图");
        assert!(icons.checked, "当前是图标视图，它应当打勾");
        assert!(!details.checked);

        // 排序子菜单里的勾必须落在当前键上，且只落在一个。
        let submenu = nodes
            .iter()
            .find_map(|node| match node {
                MenuNode::Submenu { label, items, .. } if label == "排序方式" => Some(items),
                _ => None,
            })
            .expect("空白处菜单应当有「排序方式」子菜单");

        let checked: Vec<&str> = submenu
            .iter()
            .filter_map(|node| match node {
                MenuNode::Item(spec) if spec.checked => Some(spec.label.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(checked, vec!["按大小"]);
        assert_eq!(submenu.len(), 4, "四个排序键都应当列出");
    }

    #[test]
    fn area_menu_keeps_refresh_and_hides_nothing_that_works() {
        let nodes = area_menu_spec(ViewMode::Details, SortKey::Name);
        let refresh = find(&nodes, "刷新").expect("刷新应当存在");
        assert!(!refresh.disabled, "刷新是已实现能力，不能禁用");
        assert_eq!(refresh.action, MenuAction::Refresh);

        // 新建与粘贴是写操作，位置保留但禁用。
        for label in ["新建文件夹", "新建文本文档", "粘贴"] {
            let spec = find(&nodes, label).unwrap_or_else(|| panic!("{label} 不该被删掉"));
            assert!(spec.disabled, "{label} 尚未接入，必须是禁用态");
        }
    }

    #[test]
    fn entry_menu_grouping_matches_the_design() {
        // 设计稿的分组：打开组 / 写操作组 / 属性与删除组，共两条分隔线。
        let nodes = entry_menu_spec(&directory("子目录"), 1, Some(Path::new("C:\\dir")));
        let separators = nodes
            .iter()
            .filter(|node| matches!(node, MenuNode::Separator))
            .count();
        assert_eq!(separators, 2);

        // 分隔线不应当出现在首尾——那会在菜单边缘多出一条悬空的线。
        assert!(!matches!(nodes.first(), Some(MenuNode::Separator)));
        assert!(!matches!(nodes.last(), Some(MenuNode::Separator)));

        // 最后一项是危险动作「移到回收站」。
        assert!(matches!(
            nodes.last(),
            Some(MenuNode::Item(MenuItemSpec { label, .. })) if label == "移到回收站"
        ));
    }
}
