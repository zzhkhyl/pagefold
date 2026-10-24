//! 快捷键的**交互**部分：录制浮层、改键、启停、恢复默认。
//!
//! 与 [`crate::shortcuts`] 分工：那边只负责「配置 → GPUI 键位表」的翻译，
//! 这边负责用户动作。拆开是因为两者关注点不同，也能让 `shortcuts.rs` 保持可单测
//! （不依赖 `ShellView`）。

use fm_core::KeyCombo;
use gpui::{
    AnyElement, Context, InteractiveElement as _, IntoElement as _, ParentElement as _,
    Styled as _, Window, div, px,
    prelude::FluentBuilder as _,
};
use gpui_component::WindowExt as _;

use crate::ShellView;
use crate::shortcuts::{apply, from_keystroke};

/// 录制新键位时的目标。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    /// 应用内命令（`fm_core::shortcut::COMMANDS` 里的 id）。
    Command(String),
    /// 全局唤醒热键。
    Global,
}

impl Target {
    /// 给用户看的目标名。
    pub(crate) fn label(&self) -> String {
        match self {
            Target::Command(id) => fm_core::shortcut::command(id)
                .map(|command| command.label.to_string())
                .unwrap_or_else(|| id.clone()),
            Target::Global => String::from("全局唤醒热键"),
        }
    }
}

/// 一次录制会话。
#[derive(Clone, Debug)]
pub(crate) struct Capture {
    pub target: Target,
    /// 已经按下的组合（还没确认）。
    pub combo: Option<KeyCombo>,
    /// 上一次确认失败的原因（例如撞车），显示在浮层里。
    pub error: Option<String>,
}

impl ShellView {
    /// 打开录制浮层。
    pub(crate) fn begin_shortcut_capture(
        &mut self,
        target: Target,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shortcut_capture = Some(Capture {
            target,
            combo: None,
            error: None,
        });
        // 焦点必须落在浮层上：按键事件只沿焦点路径分发，焦点在列表里就录不到。
        window.focus(&self.shortcut_focus, cx);
        cx.notify();
    }

    /// 关闭录制浮层（取消，或确认成功后）。
    pub(crate) fn cancel_shortcut_capture(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shortcut_capture = None;
        window.focus(&self.shell_focus, cx);
        cx.notify();
    }

    /// 浮层里按下了一个键。
    pub(crate) fn capture_keystroke(
        &mut self,
        keystroke: &gpui::Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.shortcut_capture.is_none() {
            return;
        }
        if keystroke.key == "escape" {
            self.cancel_shortcut_capture(window, cx);
            return;
        }

        // 只按修饰键时 `from_keystroke` 返回 `None`：那是「还没按完」，
        // 不是错误，因此不改提示、不闪烁。
        if let (Some(combo), Some(capture)) = (from_keystroke(keystroke), self.shortcut_capture.as_mut())
        {
            capture.combo = Some(combo);
            capture.error = None;
        }
        cx.notify();
    }

    /// 确认录制结果并立刻生效。
    pub(crate) fn confirm_shortcut_capture(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(capture) = self.shortcut_capture.clone() else {
            return;
        };
        let Some(combo) = capture.combo.clone() else {
            if let Some(capture) = self.shortcut_capture.as_mut() {
                capture.error = Some(String::from("先按一个组合，再点确定"));
            }
            cx.notify();
            return;
        };

        let keys = combo.binding();
        let result = match &capture.target {
            Target::Command(id) => self.assign_command_shortcut(id, &keys, cx),
            Target::Global => self.assign_global_hotkey(&keys),
        };

        match result {
            Ok(()) => {
                window.push_notification(
                    gpui_component::notification::Notification::success(format!(
                        "「{}」现在是 {}",
                        capture.target.label(),
                        combo.display()
                    )),
                    cx,
                );
                self.cancel_shortcut_capture(window, cx);
            }
            Err(reason) => {
                // 失败时**留在浮层里**：用户可以直接换一个组合再试，
                // 而不是关掉重开一遍。
                if let Some(capture) = self.shortcut_capture.as_mut() {
                    capture.error = Some(reason);
                }
                cx.notify();
            }
        }
    }

    /// 给某个命令换键位。
    fn assign_command_shortcut(
        &mut self,
        id: &str,
        keys: &str,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let combo = KeyCombo::parse(keys).ok_or_else(|| String::from("无法识别的键位"))?;
        fm_core::looks_usable_as_shortcut(&combo).map_err(str::to_string)?;

        // 撞车检查：同类快捷键之间、以及与全局热键之间。
        let items = fm_core::resolve(&self.config.shortcuts);
        if let Some(other) = fm_core::shortcut::occupant(&items, keys)
            && other.id != id
        {
            return Err(format!(
                "{} 已经分配给「{}」，先把那条禁用或改掉",
                combo.display(),
                other.label
            ));
        }
        if self.config.global_hotkey.enabled && self.config.global_hotkey.keys == keys {
            return Err(format!("{} 已经用于全局唤醒热键", combo.display()));
        }

        self.config
            .shortcuts
            .insert(id.to_string(), fm_core::ShortcutBinding::new(keys));
        self.config.save();
        // 立刻重建键位表：`apply` 内部会先 clear，因此旧键位不会残留。
        apply(cx, &self.config, &self.base_bindings);
        self.refresh_shortcut_cache();
        Ok(())
    }

    /// 换全局唤醒热键。
    fn assign_global_hotkey(&mut self, keys: &str) -> Result<(), String> {
        let combo = KeyCombo::parse(keys).ok_or_else(|| String::from("无法识别的键位"))?;

        // Win 组合对全局热键是「结构性不可用」，要给出比通用规则更具体的话：
        // 这正是用户最初报的那个问题的答案（Win+E 抢不过来）。
        if combo.win {
            return Err(String::from(
                "Win 组合被系统占用，无法注册为全局热键（请换 Ctrl / Alt 组合）",
            ));
        }
        fm_core::looks_usable_as_shortcut(&combo).map_err(str::to_string)?;

        let items = fm_core::resolve(&self.config.shortcuts);
        if let Some(other) = fm_core::shortcut::occupant(&items, keys) {
            return Err(format!(
                "{} 已经分配给「{}」，两者会互相抢",
                combo.display(),
                other.label
            ));
        }

        // 探测可用性。注意：**宿主已经占着当前这个组合时探测会失败**，
        // 那种情况其实是「我们自己的」，不能报冲突。
        let already_ours =
            self.config.global_hotkey.enabled && self.config.global_hotkey.keys == keys;
        if !already_ours {
            fm_platform::probe_hotkey(keys)?;
        }

        self.config.global_hotkey = fm_core::ShortcutBinding::new(keys);
        self.config.save();
        self.sync_global_hotkey()
    }

    /// 启用 / 禁用一条应用内快捷键。
    pub(crate) fn toggle_shortcut_enabled(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(command) = fm_core::shortcut::command(id) else {
            return;
        };
        let current = self
            .config
            .shortcuts
            .get(id)
            .map(|binding| binding.enabled)
            .unwrap_or(true);
        let keys = self
            .config
            .shortcuts
            .get(id)
            .map(|binding| binding.keys.clone())
            .unwrap_or_else(|| command.default_keys.to_string());

        self.config.shortcuts.insert(
            id.to_string(),
            fm_core::ShortcutBinding {
                keys,
                enabled: !current,
            },
        );
        self.config.save();
        apply(cx, &self.config, &self.base_bindings);
        self.refresh_shortcut_cache();

        window.push_notification(
            gpui_component::notification::Notification::success(format!(
                "「{}」已{}",
                command.label,
                if current { "禁用" } else { "启用" }
            )),
            cx,
        );
        cx.notify();
    }

    /// 启用 / 禁用全局唤醒热键。
    pub(crate) fn toggle_global_hotkey(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let enabled = !self.config.global_hotkey.enabled;
        self.config.global_hotkey.enabled = enabled;
        self.config.save();

        match self.sync_global_hotkey() {
            Ok(()) => window.push_notification(
                gpui_component::notification::Notification::success(if enabled {
                    "全局唤醒热键已启用"
                } else {
                    "全局唤醒热键已停用"
                }),
                cx,
            ),
            Err(reason) => window.push_notification(
                gpui_component::notification::Notification::error(reason),
                cx,
            ),
        }
        cx.notify();
    }

    /// 全部恢复默认（应用内 + 全局）。
    pub(crate) fn reset_shortcuts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.config.shortcuts.clear();
        self.config.global_hotkey =
            fm_core::ShortcutBinding::new(fm_core::shortcut::DEFAULT_GLOBAL_HOTKEY);
        self.config.save();
        apply(cx, &self.config, &self.base_bindings);
        self.refresh_shortcut_cache();

        match self.sync_global_hotkey() {
            Ok(()) => window.push_notification(
                gpui_component::notification::Notification::success("已恢复默认快捷键"),
                cx,
            ),
            Err(reason) => window.push_notification(
                gpui_component::notification::Notification::error(reason),
                cx,
            ),
        }
        cx.notify();
    }

    /// 把当前的全局热键同步给宿主进程。
    ///
    /// 写文件 + 发消息两步：宿主读文件拿组合（避免进程间传字符串），
    /// 消息只是「现在重读」的信号。
    fn sync_global_hotkey(&self) -> Result<(), String> {
        let keys = self
            .config
            .global_hotkey
            .enabled
            .then(|| self.config.global_hotkey.keys.clone());

        fm_platform::write_hotkey_spec(keys.as_deref())
            .map_err(|error| format!("写入热键配置失败：{error}"))?;

        if fm_platform::notify_hotkey_host() {
            return Ok(());
        }

        // 宿主没在跑：把它拉起来（它会自己读刚写好的文件）。
        fm_platform::spawn_hotkey_host();
        Ok(())
    }

    /// 录制浮层。
    ///
    /// 做成整窗遮罩而不是系统对话框：按键要落在我们自己的元素上才录得到，
    /// 而系统对话框拿不到 GPUI 的按键事件。
    pub(crate) fn render_shortcut_overlay(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        // 只在设置页渲染：即便状态没来得及清掉，也不会盖到列表上。
        if !self.in_settings() {
            return None;
        }
        let capture = self.shortcut_capture.as_ref()?;
        let tokens = &self.tokens;

        let preview = match &capture.combo {
            Some(combo) => combo.display(),
            None => String::from("按下新的组合…"),
        };
        let preview_color = if capture.combo.is_some() {
            tokens.fg
        } else {
            tokens.muted_fg
        };

        let card = div()
            .id("shortcut-capture-card")
            .flex()
            .flex_col()
            .gap(px(12.0))
            .w(px(360.0))
            .p(px(18.0))
            .rounded(px(fm_views::metrics::RADIUS_LG))
            .border_1()
            .border_color(tokens.border)
            .bg(tokens.surface)
            .text_color(tokens.fg)
            .font_family(tokens.font_family.clone())
            .shadow_lg()
            // 卡片自己吃掉鼠标按下：否则点卡片空白处会冒泡到遮罩，被当成「点击外部」关掉。
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|_, _: &gpui::MouseDownEvent, _, cx| cx.stop_propagation()),
            )
            .child(
                div()
                    .text_size(px(fm_views::metrics::FONT_LG))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(format!("修改快捷键 · {}", capture.target.label())),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(px(52.0))
                    .rounded(px(fm_views::metrics::RADIUS))
                    .border_1()
                    .border_color(tokens.border)
                    .bg(tokens.bg)
                    .text_size(px(fm_views::metrics::FONT_LG))
                    .text_color(preview_color)
                    .child(preview),
            )
            .when_some(capture.error.clone(), |element, error| {
                element.child(
                    div()
                        .text_size(px(fm_views::metrics::FONT_XS))
                        .text_color(tokens.danger)
                        .child(error),
                )
            })
            .child(
                div()
                    .text_size(px(fm_views::metrics::FONT_XS))
                    .text_color(tokens.muted_fg)
                    .child("必须是 Ctrl / Alt / Shift 加一个主键；Esc 取消。"),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .justify_end()
                    .gap(px(8.0))
                    .child(fm_views::widgets::text_button(
                        "shortcut-cancel",
                        "取消",
                        None,
                        "放弃这次修改",
                        false,
                        tokens,
                        cx.listener(|this, _: &gpui::ClickEvent, window, cx| {
                            this.cancel_shortcut_capture(window, cx);
                        }),
                    ))
                    .child(fm_views::widgets::text_button(
                        "shortcut-confirm",
                        "确定",
                        None,
                        "保存并立刻生效",
                        true,
                        tokens,
                        cx.listener(|this, _: &gpui::ClickEvent, window, cx| {
                            this.confirm_shortcut_capture(window, cx);
                        }),
                    )),
            );

        Some(
            div()
                .id("shortcut-overlay")
                .absolute()
                .inset_0()
                // 遮罩挡住底下的一切，避免录制期间误点列表。
                .occlude()
                .bg(fm_views::tokens::with_alpha(tokens.fg, 0.35))
                .flex()
                .items_center()
                .justify_center()
                .track_focus(&self.shortcut_focus)
                // 按键必须挂在**拿到焦点的这一层**上：焦点事件只沿焦点路径分发，
                // 挂在子卡片上时（焦点在遮罩）永远收不到 —— 这正是「按什么都没反应」。
                .on_key_down(cx.listener(
                    |this, event: &gpui::KeyDownEvent, window, cx| {
                        this.capture_keystroke(&event.keystroke, window, cx);
                    },
                ))
                // 点遮罩空白处 = 取消。卡片自己 stop_propagation，所以点卡片不会误关。
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|this, _: &gpui::MouseDownEvent, window, cx| {
                        this.cancel_shortcut_capture(window, cx);
                    }),
                )
                .child(card)
                .into_any_element(),
        )
    }
}
