//! 内容区视图层：设计令牌、面包屑、图标网格、详情表格、空状态与预览面板。
//!
//! **性能约束**：列表渲染必须虚拟化，复杂度为 O(视口可见行数)，与目录总条目数无关——
//! 这是「数万级条目下滚动不掉帧」的唯一可行做法。
//!
//! 虚拟化原语选用 gpui 自带的 `uniform_list`，而不是组件库的
//! `ListDelegate` / `TableDelegate`：后者的接口面很大，并且把选中语义、
//! 右键菜单、增量加载等我们尚未实现的能力一并绑了进来。`uniform_list`
//! 只要求「每项高度一致」，对本项目两种视图都能覆盖（网格按「行」虚拟化）。
//!
//! 本层不认识 `gpui_component::Theme`：配色一律通过 [`Tokens`] 传入（见 `tokens`）。

pub mod breadcrumb;
pub mod computer;
pub mod context_menu;
pub mod details_table;
pub mod empty_state;
pub mod icon_grid;
pub mod preview_panel;
pub mod tokens;
pub mod widgets;

pub use context_menu::{AreaMenuBuilder, EntryMenuBuilder, RightPressHandler};
pub use tokens::{Tokens, breakpoints, metrics};

use std::ops::Range;
use std::rc::Rc;

use fm_core::Entry;
use gpui::{
    AnyElement, App, ClickEvent, Context, ElementId, Render, Styled as _, UniformListScrollHandle,
    Window, px, uniform_list,
};

/// 单击选中项的三种语义，与资源管理器一致。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectMode {
    /// 普通单击：清空后只选中这一项。
    Replace,
    /// Ctrl / Cmd 单击：切换该项的选中状态。
    Toggle,
    /// Shift 单击：以锚点为起点扩选区间。
    Range,
}

/// 从点击事件推导选中语义。
///
/// 放在视图层是因为「哪个修饰键对应哪种选择」属于交互约定，
/// 而真正维护选中集合的是 shell —— 视图只负责把语义翻译出来。
pub fn select_mode(event: &ClickEvent) -> SelectMode {
    let modifiers = event.modifiers();
    if modifiers.shift {
        SelectMode::Range
    } else if modifiers.control || modifiers.platform {
        SelectMode::Toggle
    } else {
        SelectMode::Replace
    }
}

/// 「双击打开」回调。
///
/// 用 `Rc<dyn Fn>` 而不是泛型参数：虚拟化列表的每一行都要各自持有一个 `'static` 闭包，
/// 泛型 `F` 无法按行克隆。用 `dyn` 还省掉了一层泛型实例化。
///
/// 语义划分：视图层只负责把「某个条目被双击」这件事转出来；
/// 「是目录就进入、是文件暂不处理」的判断留在 shell —— 视图不该知道导航语义。
pub type OpenHandler = Rc<dyn Fn(Entry, &mut Window, &mut App)>;

/// 「单击选中」回调。参数是条目在当前可视列表中的下标与选择语义。
pub type SelectHandler = Rc<dyn Fn(usize, SelectMode, &mut Window, &mut App)>;

/// 用 `uniform_list` 做虚拟化的通用封装。
///
/// `row` 只在**视口可见范围内**被调用，因此可以放心在其中做格式化等有成本的运算。
/// 封装成函数而不是让各视图自己写 `cx.processor`，是因为 `uniform_list` 要求回调
/// `'static` 且返回 `Vec<R>`，在业务代码里手写这段适配很容易出错。
///
/// 行高由 `uniform_list` 实测**第一项**得出，因此换密度时只要改行内高度即可，
/// 不需要额外的「行高参数」——但所有行必须等高，否则滚动定位会漂移。
///
/// `scroll` 由调用方持有并传入：只有它知道「该把哪一条滚进视野」（新建条目之后、
/// 重命名之后）。两种视图**各自持有自己的句柄**——它们的虚拟化单位不同
/// （详情表一行一个条目，网格一行多个单元格），共用一个句柄会让切换视图时的
/// 滚动位置互相污染。
pub fn virtual_rows<T, F>(
    id: impl Into<ElementId>,
    scroll: &UniformListScrollHandle,
    cx: &Context<T>,
    count: usize,
    row: F,
) -> gpui::UniformList
where
    T: Render + 'static,
    F: Fn(&mut T, usize, &mut Window, &mut Context<T>) -> AnyElement + 'static,
{
    let render = cx.processor(move |view: &mut T, range: Range<usize>, window, cx| {
        range
            .map(|index| row(view, index, window, cx))
            .collect::<Vec<AnyElement>>()
    });

    // 弹性尺寸必须加在 `UniformList` 上：它自身只设置 `overflow.y = Scroll`，
    // 在 flex 容器里如果没人给它高度，会按内容高度计算——而虚拟列表的内容高度是 0，
    // 结果就是「不报错但什么都不显示」。
    //
    // 返回具体类型而不是 `impl IntoElement`：调用方还需要给它加内边距
    // （网格视图的 12px 外衬），不透明返回类型会让这些样式方法无法访问。
    uniform_list(id, count, render)
        .track_scroll(scroll)
        .flex_grow(1.0)
        .min_h(px(0.0))
        .w_full()
}
