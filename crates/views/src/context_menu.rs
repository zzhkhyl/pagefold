//! 右键上下文菜单的**挂载层**。
//!
//! 分工与 `open` / `select` 两个处理器完全一致：视图只负责把「在某个条目上按了右键」
//! 翻译成一个挂载点；**菜单内容**（有哪些项、哪些禁用、怎么分组）由 shell 决定——
//! 视图不认识「目录该不该有『在新标签页中打开』」这类业务语义。
//!
//! 弹出层本身不在这里实现。定位、边缘吸附、`Esc` 关闭、点击外部关闭、
//! 上下键切换与回车确认全部由 `gpui_component::menu::PopupMenu` 提供
//! （`menu::init` 已在 `gpui_component::init` 中注册）。这四件事恰是自造弹出层
//! 最容易出错的部分，没有理由重写一遍。
//!
//! 菜单底面与悬停底色取自 `gpui_component::Theme` 的 `popover` / `list_hover`
//! 字段，而 shell 的 `apply_component_theme` 已把它们同步到设计令牌，
//! 因此深浅色主题自动跟随，本层不需要也不应该再取一次颜色（视图层不认识 `Theme`）。

use std::rc::Rc;

use fm_core::Entry;
use gpui::{
    AnyElement, App, Context, Div, InteractiveElement as _, IntoElement, MouseButton,
    ParentElement as _, Stateful, Window,
};
use gpui_component::menu::{ContextMenuExt as _, PopupMenu};

/// 条目右键菜单的内容构造器。
///
/// 参数依次是：待配置的菜单、**被右键的那个条目**、它在 `visible()` 中的下标、
/// 窗口与菜单自身的上下文。
///
/// 条目与下标随菜单一起传进来，所以每个菜单项的回调天然携带了目标数据——
/// 不需要再引入一个「当前右键目标」的全局状态，也就不会有
/// 「菜单还没关、目标已经被切走」的竞态。
pub type EntryMenuBuilder =
    Rc<dyn Fn(PopupMenu, &Entry, usize, &mut Window, &mut Context<PopupMenu>) -> PopupMenu>;

/// 内容区空白处的右键菜单构造器（没有目标条目，因此不带条目参数）。
pub type AreaMenuBuilder =
    Rc<dyn Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu>;

/// 右键按下时的处理器：用于「落在未选中项上就先把它选中」。
pub type RightPressHandler = Rc<dyn Fn(usize, &mut Window, &mut App)>;

/// 给一行 / 一个网格单元挂上右键菜单。
///
/// `wrapper` 的**布局由调用方决定**，本函数只负责两件事：右键回调与菜单挂载。
/// 之所以不在这里写死尺寸：详情行需要 `w_full()`（占满整行），
/// 而网格单元格必须由内容撑开——在 `flex_row` 里给单元格写 `w_full()`
/// 会让每个单元格都占满整行，5 个单元格就是 500% 宽。
///
/// # 为什么外面要多包一层 `div`
///
/// `ContextMenu` 的右键监听挂在 `window.on_mouse_event` 的**冒泡阶段**，
/// 且它自己不调用 `stop_propagation`（源码里那唯一一处是给滚轮用的）。
/// 而内容区容器上还挂着「空白处菜单」，它是行的祖先——两者会**同时弹出**。
///
/// 这里那行 `stop_propagation` 只能截断「行内部」剩余的处理，**拦不住祖先的菜单**：
/// 窗口级监听在冒泡阶段是**逆序**执行的，而容器的监听因为「先画子元素、后注册自己」
/// 排在最前——等这个处理器被调用时，祖先的浮层早已被安排上了。真正负责收起
/// 空白处菜单的是 `ShellView::area_menu_suppressed` 标记（置位于此、消费于
/// `fm_shell::context_menu::area_menu_builder`，靠「空菜单不渲染」这一出口生效）。
pub fn entry_menu_row(
    wrapper: Stateful<Div>,
    row: Stateful<Div>,
    entry: &Entry,
    index: usize,
    menu: &EntryMenuBuilder,
    right_press: &RightPressHandler,
) -> AnyElement {
    let target = entry.clone();
    let menu = menu.clone();
    let right_press = right_press.clone();

    wrapper
        .on_mouse_down(MouseButton::Right, move |_event, window, cx| {
            (*right_press)(index, window, cx);
            // 只影响行内部剩余的处理；祖先的菜单由 `area_menu_suppressed` 负责，
            // 原因见上面的文档注释。
            cx.stop_propagation();
        })
        .child(row.context_menu(move |popup, window, cx| {
            (*menu)(popup, &target, index, window, cx)
        }))
        .into_any_element()
}
