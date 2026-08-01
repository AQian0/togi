//! 按键处理与退出命令识别。
//!
//! 将 [`Session`] 的按键分发逻辑提取到独立模块，使 `session.rs` 保持精简。
//! 通过独立的 `impl Session` 块直接访问 Session 的 `pub(crate)` 字段。

use crate::shared::constants;
use crate::tools::ApprovalDecision;
use crate::ui::complete::{self, TabCompletion};
use crate::ui::menu::MenuAction;
use crate::ui::session::Session;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::time::Instant;

/// 按键处理结果。
pub(crate) enum Action {
    Continue,
    Submit,
    Quit,
    Paste,
}

/// 判断输入是否为退出命令。
pub(crate) fn is_exit_command(input: &str) -> bool {
    matches!(input, "exit" | "quit" | "/exit" | "/quit")
}

fn approval_decision(key: &KeyEvent) -> Option<ApprovalDecision> {
    if key.kind == KeyEventKind::Repeat {
        return None;
    }
    if key.code == KeyCode::Esc {
        return Some(ApprovalDecision::Deny);
    }
    if key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
    {
        return None;
    }
    match key.code {
        KeyCode::Enter if key.modifiers.is_empty() => Some(ApprovalDecision::AllowOnce),
        KeyCode::Char('y' | 'Y') => Some(ApprovalDecision::AllowOnce),
        KeyCode::Char('a' | 'A') => Some(ApprovalDecision::AlwaysAllow),
        KeyCode::Char('n' | 'N') => Some(ApprovalDecision::Deny),
        _ => None,
    }
}

impl Session {
    /// 处理单个按键事件，返回建议的后续动作和可选的要提交的消息文本。
    pub(crate) fn handle_key(&mut self, key: KeyEvent) -> (Action, Option<String>) {
        // 菜单打开时接管全部按键。
        if self.menu.is_open() {
            return (self.menu_key(key), None);
        }
        // 任何非 Tab 键都会中断循环补全状态。
        if key.code != KeyCode::Tab {
            self.tab_completion = None;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);

        if self.has_pending_approval()
            && let Some(decision) = approval_decision(&key)
        {
            self.resolve_approval(decision);
            return (Action::Continue, None);
        }

        if key.code == KeyCode::Esc {
            if self.submitting {
                let _ = self.cancel_tx.send_replace(true);
                return (Action::Continue, None);
            }
            // 闲置时唤出悬浮菜单（长按重复事件忽略，防止菜单闪烁）。
            if key.kind != KeyEventKind::Repeat {
                self.menu.open();
            }
            return (Action::Continue, None);
        }

        if key.code == KeyCode::Char('c') && ctrl {
            if !self.submitting && self.editor.is_selecting() {
                self.editor.input(key);
                self.last_ctrl_c = None;
                return (Action::Continue, None);
            }
            let now = Instant::now();
            match self.last_ctrl_c {
                Some(last) if now.duration_since(last) <= constants::DOUBLE_PRESS_WINDOW => {
                    return (Action::Quit, None);
                }
                _ => {
                    self.last_ctrl_c = Some(now);
                    return (Action::Continue, None);
                }
            }
        }
        self.last_ctrl_c = None;

        if (key.code == KeyCode::Char('V') || key.code == KeyCode::Char('v')) && ctrl
            || (key.code == KeyCode::Char('v') && key.modifiers.contains(KeyModifiers::SUPER))
        {
            if !self.submitting {
                return (Action::Paste, None);
            }
            return (Action::Continue, None);
        }

        if key.code == KeyCode::PageUp || key.code == KeyCode::PageDown {
            if key.code == KeyCode::PageUp {
                self.conv_scroll.scroll_page_up();
            } else {
                self.conv_scroll.scroll_page_down();
            }
            return (Action::Continue, None);
        }

        if self.submitting {
            return (Action::Continue, None);
        }

        if !matches!(key.code, KeyCode::Up | KeyCode::Down) {
            self.detach_history();
        }

        match key.code {
            KeyCode::Enter => {
                if alt || ctrl || shift {
                    self.editor.insert_newline();
                } else if !self.editor.lines().iter().all(|l| l.trim().is_empty()) {
                    let trimmed = self.editor.lines().join("\n").trim().to_string();
                    if is_exit_command(&trimmed) {
                        return (Action::Quit, None);
                    }
                    self.conv.push_user_message(&trimmed);
                    self.history.push(trimmed.clone());
                    self.editor = crate::ui::editor::make_textarea("");
                    self.submitting = true;
                    self.conv_scroll.scroll_to_bottom();
                    return (Action::Submit, Some(trimmed));
                }
            }
            KeyCode::Char('j') if ctrl => self.editor.insert_newline(),
            KeyCode::Char('u') if ctrl => {
                self.editor.delete_line_by_head();
            }
            KeyCode::Char('z' | 'Z') if ctrl && shift => {
                self.editor.redo();
            }
            KeyCode::Char('z' | 'Z') if ctrl => {
                self.editor.undo();
            }
            KeyCode::Tab => self.complete_command(),
            KeyCode::Backspace if ctrl || alt => {
                self.editor.delete_word();
            }
            KeyCode::Up | KeyCode::Down if key.modifiers.is_empty() => {
                let before = self.editor.cursor();
                self.editor.input(key);
                if self.editor.cursor() == before {
                    if key.code == KeyCode::Up {
                        self.history_prev();
                    } else {
                        self.history_next();
                    }
                }
            }
            _ => {
                self.editor.input(key);
            }
        }
        (Action::Continue, None)
    }

    /// 菜单按键：Esc 关闭，上下移动，Enter 执行。
    fn menu_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Esc if key.kind != KeyEventKind::Repeat => self.menu.close(),
            KeyCode::Up => self.menu.move_up(),
            KeyCode::Down => self.menu.move_down(),
            KeyCode::Enter => match self.menu.selected_action() {
                MenuAction::Quit => return Action::Quit,
            },
            _ => {}
        }
        Action::Continue
    }

    /// Tab 补全内置命令：首个 Tab 计算候选并填入第一项，后续 Tab 循环切换。
    /// 非命令上下文（非 `/` 开头或已输入参数）退化为插入空格。
    fn complete_command(&mut self) {
        if let Some(state) = &mut self.tab_completion {
            state.index = (state.index + 1) % state.matches.len();
            let next = state.matches[state.index];
            self.editor = crate::ui::editor::make_textarea(next);
            return;
        }
        let text = self.editor.lines().join("\n");
        if !complete::is_command_context(&text) {
            self.editor.insert_tab();
            return;
        }
        let matches = complete::candidates(&text);
        match matches.len() {
            0 => {}
            1 => self.editor = crate::ui::editor::make_textarea(matches[0]),
            _ => {
                self.editor = crate::ui::editor::make_textarea(matches[0]);
                self.tab_completion = Some(TabCompletion { matches, index: 0 });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_keys_map_to_decisions() {
        assert_eq!(
            approval_decision(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Some(ApprovalDecision::AllowOnce)
        );
        assert_eq!(
            approval_decision(&KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT)),
            Some(ApprovalDecision::AlwaysAllow)
        );
        assert_eq!(
            approval_decision(&KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            Some(ApprovalDecision::Deny)
        );
    }

    #[test]
    fn modified_approval_key_is_ignored() {
        assert_eq!(
            approval_decision(&KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL)),
            None
        );
    }

    #[test]
    fn repeated_key_does_not_approve_the_next_queued_tool() {
        let mut key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        key.kind = KeyEventKind::Repeat;
        assert_eq!(approval_decision(&key), None);
    }
}
