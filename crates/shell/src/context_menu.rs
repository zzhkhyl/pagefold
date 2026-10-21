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
//! # 与设计稿的刻意差异
//!
//! 设计稿的 `.ctx-item` 定义了两样东西：
//!
//! - **快捷键提示**（`kbd`：Ctrl+C / F2 / Delete / F5 / Ctrl+V）——已照搬，
//!   但不是手写字符串，而是给 [`PopupMenuItem`] 设置 `action`，让组件库
//!   从当前键位自动解析出提示（见 [`MenuAction::shortcut`]）。键位改一处即可。
//! - **危险项配色**（`.is-danger` 的红色）。组件库的菜单项渲染没有危险态变体，
//!   而为了这一项改成自绘会连带丢掉「图标列对齐、勾选态、禁用态」等现成能力，
//!   不划算。风险的提示改由**确认对话框**承担：点「移到回收站」不会立刻删，
//!   而是先弹一个把确定按钮染成危险色的对话框（见 `ShellView::delete_selection`）。
//!   这比只把菜单项染红更有效——染红只说了「有点危险」，
//!   而对话框会说清「要把哪一项移入回收站」。

use std::path::{Path, PathBuf};
use std::rc::Rc;

use fm_core::{Entry, EntryKind, SortKey, ViewMode};
use fm_services::clipboard::ClipboardMode;
use fm_services::ops::NewItem;
use gpui::{Action, App, ClickEvent, Context, WeakEntity, Window};
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
    /// 在当前位置新建一个项目。
    Create(NewItem),
    /// 把选中项放进剪贴板（复制或剪切）。
    Clip(ClipboardMode),
    /// 把剪贴板里的内容粘贴到当前位置。
    Paste,
    /// 开始内联重命名。目标由 `dispatch` 从被点的条目上取，不放在这里——
    /// 路径在菜单打开期间可能失效，届时按下标或路径重查一次更可靠。
    Rename,
    /// 移到回收站（先弹确认框）。
    DeleteToRecycleBin,
    /// 永久删除（不进回收站，先弹确认框，且确认文案与上面那条不同）。
    DeletePermanently,
    /// 撤销上一步可逆的写操作。
    Undo,
    /// 批量重命名（前缀 + 起始序号 + 实时预览）。
    BatchRename,
}

impl MenuAction {
    /// 该菜单项对应的全局快捷键 [`Action`]。
    ///
    /// 返回 `Some` 时，`build_menu` 会把它设到 [`PopupMenuItem::action`]，
    /// 组件库便会在菜单项右侧自动渲染当前绑定的键位提示（`Kbd`）——键位改了
    /// 提示跟着变，不需要手写两份字符串。返回 `None` 表示没有快捷键
    /// （新建、排序、切换视图这类鼠标动作）。
    fn shortcut(&self) -> Option<Box<dyn Action>> {
        use crate::actions::{
            CopyItems, CutItems, DeleteItems, DeletePermanently, OpenItem, PasteItems, Refresh,
            RenameItem, Undo,
        };

        let action: Box<dyn Action> = match self {
            MenuAction::Open => Box::new(OpenItem),
            MenuAction::Refresh => Box::new(Refresh),
            MenuAction::Clip(ClipboardMode::Copy) => Box::new(CopyItems),
            MenuAction::Clip(ClipboardMode::Cut) => Box::new(CutItems),
            MenuAction::Paste => Box::new(PasteItems),
            MenuAction::Rename => Box::new(RenameItem),
            MenuAction::DeleteToRecycleBin => Box::new(DeleteItems),
            MenuAction::DeletePermanently => Box::new(DeletePermanently),
            MenuAction::Undo => Box::new(Undo),
            _ => return None,
        };
        Some(action)
    }
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

    fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    /// 置为禁用（用于「功能有，但此刻不适用」）。
    ///
    /// 例：剪贴板为空时不能粘贴。与「还没做」不同——那类项现在一律不显示，
    /// 因此不再需要「保留位置但禁用」的占位约定。
    fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// 条目右键菜单的节点清单。
///
/// 逐项对齐 `design/file-manager.html` 里 `#files` 的 `oncontextmenu`：
/// 打开 / 在新标签页中打开 /（分隔）/ 复制 / 剪切 / 重命名 /（分隔）/ 属性 / 移到回收站。
///
/// `selected` 是当前选中项数量，用于多选文案（设计稿的「复制 3 项」）。
/// `current_dir` 是当前所在目录，供「在新标签页中打开」决定落点：
/// 目录落在自身路径，文件落在当前目录——与设计稿 `addTab(...)` 的分支一致。
pub(crate) fn entry_menu_spec(
    entry: &Entry,
    selected: usize,
    current_dir: Option<&Path>,
) -> Vec<MenuNode> {
    // 驱动器是卷而不是普通条目：改名、进回收站、复制整个磁盘、以及
    // 「在它的子目录开新标签」这几件事在语义上都不成立，因此**整组隐藏**
    // 而不是显示成禁用：禁用意味着「以后会有」，而这四件事永远不会被实现。
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

    if !is_drive {
        nodes.push(MenuNode::Item(MenuItemSpec::new(
            format!("复制{count}"),
            IconName::Copy,
            MenuAction::Clip(ClipboardMode::Copy),
        )));
        nodes.push(MenuNode::Item(MenuItemSpec::new(
            format!("剪切{count}"),
            IconName::Scissors,
            MenuAction::Clip(ClipboardMode::Cut),
        )));
        // 多选时给「批量重命名」，单选时给普通「重命名」：后者对批量没有意义，
        // 而前者对单个条目也没有意义（前缀 + 序号只有一个结果）。
        nodes.push(MenuNode::Item(if multi {
            MenuItemSpec::new("批量重命名…", IconName::Pencil, MenuAction::BatchRename)
        } else {
            MenuItemSpec::new("重命名", IconName::Pencil, MenuAction::Rename)
        }));

        // 第二条分隔线只在这组写操作存在时才加，否则驱动器菜单上会出现
        // 两条相邻的分隔线（一条悬空的横线）。
        nodes.push(MenuNode::Separator);
    }

    // 设计稿的「属性」不是独立的属性对话框，而是把右侧预览面板打开。
    nodes.push(MenuNode::Item(MenuItemSpec::new(
        "属性",
        IconName::Info,
        MenuAction::ShowPreview,
    )));

    if !is_drive {
        nodes.push(MenuNode::Item(MenuItemSpec::new(
            "移到回收站",
            IconName::Trash,
            MenuAction::DeleteToRecycleBin,
        )));
        // 永久删除必须与回收站删除**分成两项**：它们的确认文案、可逆性、
        // 快捷键都不同，合成一个「按 Shift 改变行为」的隐式动作，
        // 用户就没有地方读到「这一下删了捡不回来」。
        nodes.push(MenuNode::Item(MenuItemSpec::new(
            "永久删除",
            IconName::Eraser,
            MenuAction::DeletePermanently,
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
/// `writable` 表示**当前位置能不能落笔**：「此电脑」与回收站都不是真实目录，
/// 那里既不显示「新建」也不显示「粘贴」——与资源管理器一致。
/// 把它们渲染成一排禁用项只会让人去猜为什么点不动。
///
/// `can_paste` 是剪贴板里有没有内容。这一项**保留并禁用**（而不是隐藏）：
/// 用户刚复制过、切了个目录想来粘贴，此时菜单里能看到「粘贴」，
/// 而不是怀疑自己刚才那下复制没生效。
///
/// 设计稿末尾那段「回收站 / 清空回收站」仍没有对应实现：回收站这个虚拟位置
/// 现在已经有了（侧边栏可进入、只读浏览），但**清空回收站是写操作**，
/// 不在本次范围。
///
/// 视图模式两项的措辞取自命令条的 tooltip（「详细信息视图」/「图标视图」），
/// 而不是设计稿空白菜单里的「列表视图 / 网格视图」：同一个动作在应用内应该只有一个名字。
pub(crate) fn area_menu_spec(
    view_mode: ViewMode,
    sort_key: SortKey,
    writable: bool,
    can_paste: bool,
    undo_label: Option<&str>,
) -> Vec<MenuNode> {
    let view_node = |label: &str, icon: IconName, mode: ViewMode| {
        MenuNode::Item(
            MenuItemSpec::new(label, icon, MenuAction::SetViewMode(mode)).checked(view_mode == mode),
        )
    };
    let sort_node = |label: &str, icon: IconName, key: SortKey| {
        MenuNode::Item(MenuItemSpec::new(label, icon, MenuAction::SetSort(key)).checked(sort_key == key))
    };

    let mut nodes = Vec::new();

    // 撤销放在最顶上：它是「我刚才做错了」的第一反应，
    // 也是唯一一个在**任意目录**都有意义的写操作。
    // 没有可撤销的东西时整项隐藏（而不是禁用）：禁用意味着「以后会有」，
    // 而这里只是「现在没有」。
    if let Some(label) = undo_label {
        nodes.push(MenuNode::Item(MenuItemSpec::new(
            format!("撤销 {label}"),
            IconName::Undo2,
            MenuAction::Undo,
        )));
        nodes.push(MenuNode::Separator);
    }

    if writable {
        nodes.push(MenuNode::Item(MenuItemSpec::new(
            "新建文件夹",
            IconName::Folder,
            MenuAction::Create(NewItem::Folder),
        )));
        // 设计稿的空白处菜单只有这两项（`新建 Markdown 文档` 只出现在命令条的
        // 「新建」下拉里），这里跟随设计稿。
        nodes.push(MenuNode::Item(MenuItemSpec::new(
            "新建文本文档",
            IconName::FileText,
            MenuAction::Create(NewItem::TextFile),
        )));
        nodes.push(MenuNode::Separator);
        nodes.push(MenuNode::Item(
            MenuItemSpec::new("粘贴", IconName::ClipboardPaste, MenuAction::Paste)
                .disabled(!can_paste),
        ));
    }

    nodes.push(MenuNode::Item(MenuItemSpec::new(
        "刷新",
        IconName::RefreshCw,
        MenuAction::Refresh,
    )));
    nodes.push(MenuNode::Separator);
    // 排序收进子菜单：设计稿把它放在命令条的按钮上，这里用子菜单承载同一组动作，
    // 好处是主菜单不必变长，且当前排序键能用勾选态直接标出来。
    nodes.push(MenuNode::Submenu {
        label: String::from("排序方式"),
        icon: IconName::SortAscending,
        items: vec![
            sort_node("按名称", IconName::FileText, SortKey::Name),
            sort_node("按修改日期", IconName::Clock, SortKey::Modified),
            sort_node("按类型", IconName::Folder, SortKey::Kind),
            sort_node("按大小", IconName::List, SortKey::Size),
        ],
    });
    nodes.push(MenuNode::Separator);
    nodes.push(view_node("详细信息视图", IconName::List, ViewMode::Details));
    nodes.push(view_node("图标视图", IconName::LayoutGrid, ViewMode::Icons));

    nodes
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
                // 有全局快捷键的项，让组件库在右侧渲染键位提示（Ctrl+C / F2 …）。
                if let Some(action) = spec.action.shortcut() {
                    item = item.action(action);
                }
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
    move |_event: &ClickEvent, window: &mut Window, cx: &mut App| {
        dispatch(&shell, &action, entry.as_ref(), window, cx);
    }
}

/// 执行一个菜单动作。
///
/// 一律经 `WeakEntity::update` 回到 `ShellView`，与 `open_handler` / `select_handler` 一致：
/// 菜单的生命周期可能比视图长（视图被销毁时菜单仍可能显示在屏幕上），不能持强引用。
///
/// 写操作需要 `&mut Window`：它们都要弹提示（`push_notification` 挂在窗口上），
/// 而确认对话框与内联输入框更是必须拿到窗口句柄。
fn dispatch(
    shell: &WeakEntity<ShellView>,
    action: &MenuAction,
    entry: Option<&Entry>,
    window: &mut Window,
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
        // 写操作的目标一律取自**当前选中集**，而不是菜单构造时捕获的那一项：
        // 菜单打开期间用户可能改了选中（比如正好点了别处），
        // 而「复制/剪切/删除」的语义本来就是「对选中项」，不是「对右键点中的那一项」。
        //
        // 唯一例外是重命名：它作用于右键点中的那一个条目，因此这里单独取 `entry`。
        MenuAction::Create(item) => view.create(*item, window, cx),
        MenuAction::Clip(mode) => view.clip_selection(*mode, window, cx),
        MenuAction::Paste => view.paste(window, cx),
        MenuAction::Rename => {
            if let Some(entry) = entry {
                view.begin_rename(entry.path.as_ref().clone(), window, cx);
            }
        }
        MenuAction::DeleteToRecycleBin => view.delete_selection(window, cx),
        MenuAction::BatchRename => view.open_batch_rename(window, cx),
        MenuAction::DeletePermanently => view.delete_selection_forever(window, cx),
        MenuAction::Undo => view.undo_last(window, cx),
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
        // 这次右键落在条目上吗？是的话条目自己的菜单负责，这里必须交出一个
        // **空菜单**：组件库看到空菜单就不渲染浮层（`ContextMenu::request_layout`
        // 里的 `is_empty()` 分支）。这是唯一可靠的截断点——在行的处理器里调
        // `stop_propagation` 拦不住祖先，原因见 `right_press_handler`。
        //
        // 标记「消费即清」：否则上一次在条目上的右键会连下一次在空白处的右键一起吞掉。
        let suppressed = shell
            .update(cx, |view, _| std::mem::take(&mut view.area_menu_suppressed))
            .unwrap_or(false);
        if suppressed {
            return menu;
        }

        let Ok((view_mode, sort_key, writable, can_paste, undo_label)) =
            shell.read_with(&**cx, |view: &ShellView, _| {
                let tab = view.tabs.active_state();
                (
                    tab.view_mode(),
                    tab.sort_key(),
                    // 「此电脑」与回收站都不是真实目录，那里无处落笔。
                    view.current_dir().is_some(),
                    view.can_paste(),
                    // 菜单里直接写出「要撤哪一步」，避免用户按下去才知道。
                    view.undo.peek().map(|action| action.describe()),
                )
            })
        else {
            return menu;
        };

        let nodes = area_menu_spec(
            view_mode,
            sort_key,
            writable,
            can_paste,
            undo_label.as_deref(),
        );
        build_menu(menu, &nodes, None, &shell, window, cx)
    })
}

/// 命令条「新建」下拉的节点清单。
///
/// 与空白处菜单**刻意不同**：这里多一项「新建 Markdown 文档」，且没有
/// 刷新 / 排序 / 视图模式——那三样在命令条上已经各有自己的按钮，重复一遍
/// 只会让同一个动作出现两个入口。
///
/// 逐项对齐设计稿 `$('#btnNew').onclick` 里那次 `openCtx(...)` 调用的清单。
pub(crate) fn new_menu_spec(can_paste: bool) -> Vec<MenuNode> {
    vec![
        MenuNode::Item(MenuItemSpec::new(
            "新建文件夹",
            IconName::Folder,
            MenuAction::Create(NewItem::Folder),
        )),
        MenuNode::Item(MenuItemSpec::new(
            "新建文本文档",
            IconName::FileText,
            MenuAction::Create(NewItem::TextFile),
        )),
        MenuNode::Item(MenuItemSpec::new(
            "新建 Markdown 文档",
            IconName::FileText,
            MenuAction::Create(NewItem::MarkdownFile),
        )),
        MenuNode::Separator,
        MenuNode::Item(
            MenuItemSpec::new("粘贴", IconName::ClipboardPaste, MenuAction::Paste)
                .disabled(!can_paste),
        ),
    ]
}

/// 构造命令条「新建」下拉的菜单构造器。
///
/// 与右键菜单共用 [`build_menu`]：同一套节点、同一套回调，
/// 因此措辞与禁用逻辑不会在两个入口之间分叉。
pub(crate) fn new_menu_builder(
    cx: &Context<ShellView>,
) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
    let shell = cx.entity().downgrade();
    move |menu, window, cx| {
        // 与空白处菜单同源：剪贴板空不空决定「粘贴」是否禁用。
        let can_paste = shell
            .read_with(&**cx, |view: &ShellView, _| view.can_paste())
            .unwrap_or(false);
        let nodes = new_menu_spec(can_paste);
        build_menu(menu, &nodes, None, &shell, window, cx)
    }
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
            // 先记下「这次右键落在条目上」，让空白处菜单把自己收起来（见
            // `area_menu_builder`）。必须放在分支**之前**：无论这一项此前是否已选中，
            // 祖先的那次浮层都已经被同一个事件安排上了。
            //
            // 顺带说明为什么不能靠 `cx.stop_propagation()`：所有鼠标监听都注册在
            // `window.on_mouse_event` 上，冒泡阶段**逆序**执行，而内容区容器的监听
            // 因为「先画子元素、后注册自己」排在最前——等这里被调用时，祖先的浮层
            // 早就弹出来了，此时截断已经晚了。
            view.area_menu_suppressed = true;

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
            hidden: false,
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

    fn find<'a>(nodes: &'a [MenuNode], label: &str) -> Option<&'a MenuItemSpec> {
        nodes.iter().find_map(|node| match node {
            MenuNode::Item(spec) if spec.label == label => Some(spec),
            _ => None,
        })
    }

    /// 四项写操作必须**可用**，且各自指向正确的动作。
    ///
    /// 这组断言是「写操作接入」这件事的防回归点：它们曾经全是禁用态，
    /// 若哪天因为重构又被置回禁用，这里会立刻红。
    fn assert_write_ops_enabled(nodes: &[MenuNode]) {
        let expected = [
            ("复制", MenuAction::Clip(ClipboardMode::Copy)),
            ("剪切", MenuAction::Clip(ClipboardMode::Cut)),
            ("重命名", MenuAction::Rename),
            ("移到回收站", MenuAction::DeleteToRecycleBin),
            ("永久删除", MenuAction::DeletePermanently),
        ];
        for (label, action) in expected {
            let spec = find(nodes, label).unwrap_or_else(|| panic!("{label} 不该被删掉"));
            assert!(!spec.disabled, "{label} 已实现，不能再是禁用态");
            assert_eq!(spec.action, action, "{label} 指向了错误的动作");
        }
    }

    /// 回收站删除与永久删除必须是**两个独立项**。
    ///
    /// 合成一个「按 Shift 改变行为」的隐式动作是常见的省事做法，代价是
    /// 用户没有地方读到「这一下删了捡不回来」——而这两件事的后果差别极大。
    #[test]
    fn recycle_and_permanent_delete_are_separate_entries() {
        let nodes = entry_menu_spec(&file("报告.docx"), 1, Some(Path::new("C:\\dir")));
        let recycle = find(&nodes, "移到回收站").expect("应有「移到回收站」");
        let permanent = find(&nodes, "永久删除").expect("应有「永久删除」");

        assert_ne!(recycle.action, permanent.action);
        assert_eq!(permanent.action, MenuAction::DeletePermanently);
        assert!(!permanent.disabled);
        // 两项都带快捷键提示（Delete / Shift+Delete），否则用户只会用其中一种。
        assert!(recycle.action.shortcut().is_some());
        assert!(permanent.action.shortcut().is_some());
    }

    #[test]
    fn drives_offer_neither_kind_of_delete() {
        let nodes = entry_menu_spec(&drive("C:"), 1, Some(Path::new("C:\\")));
        assert!(find(&nodes, "移到回收站").is_none());
        assert!(find(&nodes, "永久删除").is_none(), "驱动器不能被删除");
    }

    #[test]
    fn undo_is_listed_only_when_something_can_be_undone() {
        // 空栈：整项隐藏。禁用态会让人以为「以后会有」，而这里只是「现在没有」。
        let empty = area_menu_spec(ViewMode::Details, SortKey::Name, true, true, None);
        assert!(find(&empty, "撤销").is_none());

        // 有可撤销操作时，菜单里直接写出「撤的是哪一步」。
        let nodes = area_menu_spec(
            ViewMode::Details,
            SortKey::Name,
            true,
            true,
            Some("重命名「报告.docx」"),
        );
        let spec = find(&nodes, "撤销 重命名「报告.docx」").expect("应出现带描述的撤销项");
        assert_eq!(spec.action, MenuAction::Undo);
        assert!(!spec.disabled);
        assert!(spec.action.shortcut().is_some(), "撤销应当提示 Ctrl+Z");
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
        assert_write_ops_enabled(&nodes);
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
        // 文件与目录在这一组上完全一致：都是「复制/剪切/重命名/移到回收站」。
        assert_write_ops_enabled(&nodes);
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

        // 隐藏（而不是禁用）：这几件事对「卷」永远不会有实现。
        assert!(find(&nodes, "重命名").is_none(), "驱动器不该出现改名");
        assert!(find(&nodes, "移到回收站").is_none(), "驱动器不该进回收站");
        assert!(find(&nodes, "在新标签页中打开").is_none());
        assert!(find(&nodes, "复制").is_none(), "复制整个磁盘没有意义");
        assert!(find(&nodes, "剪切").is_none());

        // 仍然保留了「打开」与「属性」。
        assert!(find(&nodes, "打开").is_some());
        assert!(find(&nodes, "属性").is_some());

        // 写操作整组被去掉之后，第二条分隔线也必须一起去掉，
        // 否则驱动器菜单上会出现两条相邻的横线（视觉上是多余的）。
        let separators = nodes
            .iter()
            .filter(|node| matches!(node, MenuNode::Separator))
            .count();
        assert_eq!(separators, 1);
    }

    #[test]
    fn multi_selection_changes_the_wording() {
        let dir = directory("子目录");
        let single = entry_menu_spec(&dir, 1, None);
        assert_eq!(find(&single, "复制").map(|s| s.label.clone()), Some(String::from("复制")));
        assert!(find(&single, "重命名").is_some());

        let many = entry_menu_spec(&dir, 3, None);
        let copy = find(&many, "复制 3 项").expect("多选时文案要带上数量");
        let cut = find(&many, "剪切 3 项").expect("剪切同理");
        assert!(!copy.disabled && !cut.disabled, "多选不影响复制/剪切的可用性");
        assert_eq!(copy.action, MenuAction::Clip(ClipboardMode::Copy));
        assert_eq!(cut.action, MenuAction::Clip(ClipboardMode::Cut));

        // 多选时给「批量重命名…」（带省略号，表示会弹出面板），单选时给普通重命名。
        let batch = find(&many, "批量重命名…").expect("多选时应提供批量重命名");
        assert!(!batch.disabled, "批量重命名已实现，必须可用");
        assert_eq!(batch.action, MenuAction::BatchRename);
        assert!(find(&many, "重命名").is_none(), "多选不该再显示单项重命名");
        assert!(
            find(&single, "批量重命名…").is_none(),
            "单选时批量重命名没有意义"
        );
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
        let nodes = area_menu_spec(ViewMode::Icons, SortKey::Size, true, true, None);

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
    fn area_menu_offers_the_create_actions_in_a_writable_directory() {
        let nodes = area_menu_spec(ViewMode::Details, SortKey::Name, true, false, None);

        let refresh = find(&nodes, "刷新").expect("刷新应当存在");
        assert!(!refresh.disabled, "刷新是已实现能力，不能禁用");
        assert_eq!(refresh.action, MenuAction::Refresh);

        let folder = find(&nodes, "新建文件夹").expect("真实目录里应当能新建");
        assert!(!folder.disabled);
        assert_eq!(folder.action, MenuAction::Create(NewItem::Folder));

        let text = find(&nodes, "新建文本文档").expect("同上");
        assert!(!text.disabled);
        assert_eq!(text.action, MenuAction::Create(NewItem::TextFile));

        // 剪贴板为空时粘贴**保留但禁用**——用户刚复制过、切了目录想来粘贴，
        // 看到这一项在（只是灰着）比整项消失更好理解。
        let paste = find(&nodes, "粘贴").expect("粘贴不该被删掉");
        assert!(paste.disabled, "can_paste = false 时必须禁用");
        assert_eq!(paste.action, MenuAction::Paste);
    }

    #[test]
    fn area_menu_enables_paste_once_the_clipboard_has_content() {
        let nodes = area_menu_spec(ViewMode::Details, SortKey::Name, true, true, None);
        let paste = find(&nodes, "粘贴").expect("粘贴应当存在");
        assert!(!paste.disabled);
        assert_eq!(paste.action, MenuAction::Paste);
    }

    #[test]
    fn area_menu_drops_the_write_group_outside_a_real_directory() {
        // 「此电脑」与回收站都不是真实目录：那里无处落笔，
        // 与其摆一排禁用项让人猜，不如整组不出现（资源管理器也是这样）。
        let nodes = area_menu_spec(ViewMode::Details, SortKey::Name, false, true, None);

        for label in ["新建文件夹", "新建文本文档", "粘贴"] {
            assert!(find(&nodes, label).is_none(), "{label} 不该出现在虚拟位置");
        }

        // 读操作与视图操作必须全部保留。
        assert!(find(&nodes, "刷新").is_some());
        assert!(find(&nodes, "详细信息视图").is_some());
        assert!(find(&nodes, "图标视图").is_some());
        assert!(
            nodes
                .iter()
                .any(|node| matches!(node, MenuNode::Submenu { label, .. } if label == "排序方式")),
            "排序子菜单与写操作无关，必须保留"
        );

        // 首项不能是分隔线：整组被去掉后如果不补，菜单会以一条横线开头。
        assert!(!matches!(nodes.first(), Some(MenuNode::Separator)));
    }

    #[test]
    fn new_menu_offers_three_create_kinds_in_the_design_order() {
        let nodes = new_menu_spec(false);

        let labels: Vec<String> = nodes
            .iter()
            .filter_map(|node| match node {
                MenuNode::Item(spec) => Some(spec.label.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            labels,
            vec![
                "新建文件夹",
                "新建文本文档",
                "新建 Markdown 文档",
                "粘贴"
            ],
            "顺序与设计稿 `$('#btnNew').onclick` 的清单一致"
        );

        assert_eq!(
            find(&nodes, "新建 Markdown 文档").map(|spec| spec.action.clone()),
            Some(MenuAction::Create(NewItem::MarkdownFile)),
            "「新建 Markdown 文档」只出现在命令条的下拉里"
        );

        // 下拉里不该出现刷新 / 排序 / 视图模式：命令条上它们各有自己的按钮。
        for label in ["刷新", "详细信息视图", "图标视图", "排序方式"] {
            assert!(
                find(&nodes, label).is_none(),
                "{label} 不该出现在「新建」下拉里"
            );
        }
    }

    #[test]
    fn new_menu_disables_paste_until_the_clipboard_has_content() {
        let empty = new_menu_spec(false);
        let paste = find(&empty, "粘贴").expect("粘贴应当存在");
        assert!(paste.disabled, "剪贴板为空时必须禁用");
        assert_eq!(paste.action, MenuAction::Paste);

        let filled = new_menu_spec(true);
        let paste = find(&filled, "粘贴").expect("粘贴应当存在");
        assert!(!paste.disabled, "有内容时必须可用");
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

        // 末尾两项是危险动作，顺序是「轻的在前、重的在后」：
        // 回收站可反悔，永久删除不可——把它放在最后，误触需要多按一次。
        let labels: Vec<&str> = nodes
            .iter()
            .filter_map(|node| match node {
                MenuNode::Item(spec) => Some(spec.label.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            &labels[labels.len() - 2..],
            &["移到回收站", "永久删除"],
            "危险动作的排列顺序变了：{labels:?}"
        );
    }
}
