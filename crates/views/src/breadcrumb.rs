//! 面包屑路径导航。
//!
//! 形态照搬设计稿的 `.crumb`：24px 高、5px 圆角、当前层为正文色、其余层级为次级色，
//! 悬停时铺一层浅灰底。「点击空白处切换为可编辑地址栏」由 shell 层配合输入框完成
//! （见 `fm-shell::toolbar`），本模块只负责逐级点击跳转。

use std::path::{Component, Path, PathBuf};
use std::rc::Rc;

use gpui::{
    AnyElement, App, ClickEvent, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use gpui_component::{Icon, Sizable as _};
use gpui_kit_assets::IconName;

use crate::tokens::{Tokens, metrics};

/// 面包屑的一段。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    /// 展示文本。
    pub label: String,
    /// 点击后应跳转到的绝对路径。
    pub path: PathBuf,
}

/// 折叠后的展示项。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Crumb {
    Segment(Segment),
    /// 被折叠掉的中间层级。
    Ellipsis,
}

/// 路径层数超过该值时折叠中间层级。
pub const MAX_VISIBLE_SEGMENTS: usize = 5;

/// 窄屏（≤720px）下的可见层级数：首段 + 省略号 + 末段。
///
/// 设计稿在窄屏下直接隐藏所有中间层级，这里保留一个省略号，
/// 让用户仍能看出「中间还有层级」，而不是误以为路径就是这两级。
pub const COMPACT_VISIBLE_SEGMENTS: usize = 3;

/// 把绝对路径切成逐级可点击的段。
///
/// 例：`C:\Users\abc` → `C:\` / `Users` / `abc`。
/// 盘符与根目录合成一段（否则会出现只有一个「C:」的孤立段），
/// `.` 与 `..` 直接丢弃——绝对路径里它们没有展示意义。
pub fn segments(path: &Path) -> Vec<Segment> {
    let mut result = Vec::new();
    let mut accumulated = PathBuf::new();

    for component in path.components() {
        match component {
            // 盘符或 UNC 前缀：先累积，等根目录到达时再一起作为首段。
            Component::Prefix(_) => accumulated.push(component),
            Component::RootDir => {
                accumulated.push(component);
                result.push(Segment {
                    label: accumulated.display().to_string(),
                    path: accumulated.clone(),
                });
            }
            Component::CurDir | Component::ParentDir => {}
            Component::Normal(name) => {
                accumulated.push(name);
                result.push(Segment {
                    label: name.to_string_lossy().into_owned(),
                    path: accumulated.clone(),
                });
            }
        }
    }

    // 相对路径（没有根目录）时上面一段都不会产出，把累积到的部分补成首段，
    // 否则面包屑会整个消失。
    if result.is_empty() && !accumulated.as_os_str().is_empty() {
        result.push(Segment {
            label: accumulated.display().to_string(),
            path: accumulated,
        });
    }

    result
}

/// 把段列表压缩到最多 `max` 项。
///
/// 超出时保留**首段与末段**、中间折叠成一个省略号：
/// 深层目录下用户最关心的就是「从哪来」和「当前在哪」，中间层级价值最低。
pub fn fold(segments: &[Segment], max: usize) -> Vec<Crumb> {
    if max == 0 {
        return Vec::new();
    }
    if segments.len() <= max {
        return segments.iter().cloned().map(Crumb::Segment).collect();
    }
    // 至少要有「首段 + 省略号 + 末段」三格才有意义，否则退化为直接截断。
    if max < 3 {
        return segments
            .iter()
            .rev()
            .take(max)
            .rev()
            .cloned()
            .map(Crumb::Segment)
            .collect();
    }

    // 首段与省略号各占一格，其余槽位全部留给尾部。
    let tail = max - 2;
    let mut result = Vec::with_capacity(max);
    result.push(Crumb::Segment(segments[0].clone()));
    result.push(Crumb::Ellipsis);
    result.extend(
        segments[segments.len() - tail..]
            .iter()
            .cloned()
            .map(Crumb::Segment),
    );
    result
}

/// 渲染整条面包屑。
///
/// `on_navigate` 会在点击某一段时被调用，参数是**该段对应的绝对路径**。
pub fn render<F>(crumbs: &[Crumb], tokens: &Tokens, on_navigate: F) -> AnyElement
where
    F: Fn(PathBuf, &mut Window, &mut App) + 'static,
{
    // 用 Rc 共享回调：每段需要一个独立的 'static 闭包，直接克隆 F 需要 F: Clone，
    // 而 Rc 只增加引用计数。
    let on_navigate = Rc::new(on_navigate);
    let last = crumbs.len().saturating_sub(1);
    let hover = tokens.hover;
    let foreground = tokens.fg;

    let children = crumbs.iter().enumerate().map(|(index, crumb)| {
        let separator = (index > 0).then(|| {
            div()
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .w(px(14.0))
                .text_color(tokens.muted_fg)
                .child(Icon::new(IconName::ChevronRight).with_size(px(11.0)))
        });

        let body = match crumb {
            // 省略号不可点击：它代表被折叠的层级，没有单一目标路径。
            Crumb::Ellipsis => div()
                .flex_none()
                .px(px(4.0))
                .text_color(tokens.muted_fg)
                .child("…")
                .into_any_element(),
            Crumb::Segment(segment) => {
                let callback = Rc::clone(&on_navigate);
                let path = segment.path.clone();
                let is_current = index == last;
                div()
                    .id(("crumb", index))
                    .flex_none()
                    .flex()
                    .items_center()
                    .h(px(metrics::CRUMB_HEIGHT))
                    .px(px(7.0))
                    .rounded(px(metrics::CRUMB_RADIUS))
                    .overflow_hidden()
                    .cursor_pointer()
                    .text_size(px(12.5))
                    .text_color(if is_current {
                        foreground
                    } else {
                        tokens.muted_fg
                    })
                    .hover(move |style| style.bg(hover).text_color(foreground))
                    .on_click(move |_event: &ClickEvent, window, cx| {
                        // 阻止冒泡：地址栏容器把「点空白处」当作进入编辑态的入口，
                        // 若不拦截，点某一级跳转的同时会顺带打开输入框。
                        cx.stop_propagation();
                        (*callback)(path.clone(), window, cx);
                    })
                    .child(
                        div()
                            .overflow_hidden()
                            .truncate()
                            .child(segment.label.clone()),
                    )
                    .into_any_element()
            }
        };

        div()
            .flex()
            .flex_row()
            .items_center()
            .flex_none()
            .children(separator)
            .child(body)
            .into_any_element()
    });

    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(1.0))
        .flex_grow(1.0)
        .min_w(px(0.0))
        .h_full()
        .overflow_hidden()
        .children(children)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(label: &str) -> Segment {
        Segment {
            label: label.to_string(),
            path: PathBuf::from(label),
        }
    }

    #[test]
    fn windows_path_is_split_into_drive_and_names() {
        let segments = segments(Path::new(r"C:\Users\abc"));
        let labels: Vec<&str> = segments.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, vec!["C:\\", "Users", "abc"]);
    }

    #[test]
    fn drive_root_is_a_single_segment() {
        let segments = segments(Path::new(r"C:\"));
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].label, "C:\\");
        assert_eq!(segments[0].path, PathBuf::from(r"C:\"));
    }

    #[test]
    fn each_segment_carries_the_cumulative_path() {
        let segments = segments(Path::new(r"C:\a\b"));
        assert_eq!(segments[1].path, PathBuf::from(r"C:\a"));
        assert_eq!(segments[2].path, PathBuf::from(r"C:\a\b"));
    }

    #[test]
    fn dot_components_are_dropped() {
        let segments = segments(Path::new(r"C:\a\.\b"));
        let labels: Vec<&str> = segments.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, vec!["C:\\", "a", "b"]);
    }

    #[test]
    fn short_paths_are_not_folded() {
        let items = vec![segment("C:\\"), segment("a")];
        let folded = fold(&items, MAX_VISIBLE_SEGMENTS);
        assert_eq!(folded.len(), 2);
        assert!(
            folded
                .iter()
                .all(|crumb| matches!(crumb, Crumb::Segment(_)))
        );
    }

    #[test]
    fn long_paths_fold_the_middle_and_keep_the_tail() {
        let items: Vec<Segment> = (0..8).map(|index| segment(&index.to_string())).collect();
        let folded = fold(&items, MAX_VISIBLE_SEGMENTS);

        assert_eq!(folded.len(), MAX_VISIBLE_SEGMENTS);
        assert_eq!(folded[0], Crumb::Segment(items[0].clone()));
        assert_eq!(folded[1], Crumb::Ellipsis);
        // 末两段必须完整保留。
        assert_eq!(folded[folded.len() - 2], Crumb::Segment(items[6].clone()));
        assert_eq!(folded[folded.len() - 1], Crumb::Segment(items[7].clone()));
    }

    #[test]
    fn compact_fold_keeps_first_ellipsis_and_last() {
        let items: Vec<Segment> = (0..6).map(|index| segment(&index.to_string())).collect();
        let folded = fold(&items, COMPACT_VISIBLE_SEGMENTS);

        assert_eq!(folded.len(), 3);
        assert_eq!(folded[0], Crumb::Segment(items[0].clone()));
        assert_eq!(folded[1], Crumb::Ellipsis);
        assert_eq!(folded[2], Crumb::Segment(items[5].clone()));
    }

    #[test]
    fn degenerate_max_falls_back_to_truncation() {
        let items: Vec<Segment> = (0..5).map(|index| segment(&index.to_string())).collect();
        let folded = fold(&items, 2);
        assert_eq!(folded.len(), 2);
        // max 太小时保留末尾（离当前位置最近），保证用户仍能看到「我在哪」。
        assert_eq!(folded[0], Crumb::Segment(items[3].clone()));
        assert_eq!(folded[1], Crumb::Segment(items[4].clone()));
    }
}
