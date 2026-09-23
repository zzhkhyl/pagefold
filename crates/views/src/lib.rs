//! 内容区视图层：面包屑、图标网格、详情表格、空状态。
//!
//! **性能约束**：列表渲染必须虚拟化，复杂度为 O(视口可见行数)，与目录总条目数无关——
//! 这是「数万级条目下滚动不掉帧」的唯一可行做法。
//!
//! 虚拟化原语选用 gpui 自带的 `uniform_list`，而不是组件库的
//! `ListDelegate` / `TableDelegate`：后者的接口面很大，并且把选中语义、
//! 右键菜单、增量加载等我们尚未实现的能力一并绑了进来。`uniform_list`
//! 只要求「每项高度一致」，对本项目两种视图都能覆盖（网格按“行”虚拟化）。

pub mod breadcrumb;
pub mod computer;
pub mod details_table;
pub mod empty_state;
pub mod icon_grid;

use std::ops::Range;
use std::rc::Rc;

use fm_core::Entry;
use gpui::{
    AnyElement, App, Context, ElementId, Hsla, IntoElement, Render, Styled as _, Window, px,
    uniform_list,
};

/// 视图层用到的语义色。
///
/// 集中成一个结构体传递，避免每个渲染函数都挂四五个 `Hsla` 参数；
/// 也顺带把「哪一层负责从主题取色」这件事收敛到 shell 一处。
#[derive(Clone, Copy, Debug)]
pub struct Colors {
    pub foreground: Hsla,
    pub muted_foreground: Hsla,
    pub muted: Hsla,
    pub border: Hsla,
    pub accent: Hsla,
    pub accent_foreground: Hsla,
    pub list_hover: Hsla,
}

/// 「双击打开」回调。
///
/// 用 `Rc<dyn Fn>` 而不是泛型参数：虚拟化列表的每一行都要各自持有一个 `'static` 闭包，
/// 泛型 `F` 无法按行克隆。用 `dyn` 还省掉了一层泛型实例化。
///
/// 语义划分：视图层只负责把「某个条目被双击」这件事转出来；
/// 「是目录就进入、是文件暂不处理」的判断留在 shell —— 视图不该知道导航语义。
pub type OpenHandler = Rc<dyn Fn(Entry, &mut Window, &mut App)>;

/// 用 `uniform_list` 做虚拟化的通用封装。
///
/// `row` 只在**视口可见范围内**被调用，因此可以放心在其中做格式化等有成本的运算。
/// 封装成函数而不是让各视图自己写 `cx.processor`，是因为 `uniform_list` 要求回调
/// `'static` 且返回 `Vec<R>`，在业务代码里手写这段适配很容易出错。
pub fn virtual_rows<T, F>(
    id: impl Into<ElementId>,
    cx: &Context<T>,
    count: usize,
    row: F,
) -> impl IntoElement
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
    uniform_list(id, count, render)
        .flex_grow(1.0)
        .min_h(px(0.0))
        .w_full()
}
