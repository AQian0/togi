//! 按键处理与退出命令识别。
//!
//! 将 [`Session`] 的按键分发逻辑提取到独立模块，使 `session.rs` 保持精简。
//! 通过独立的 `impl Session` 块直接访问 Session 的 `pub(crate)` 字段。

use crate::shared::constants;
use crate::ui::complete::{self, TabCompletion};
use crate::ui::session::Session;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
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

impl Session {
    /// 处理单个按键事件，返回建议的后续动作和可选的要提交的消息文本。
    pub(crate) fn handle_key(&mut self, key: KeyEvent) -> (Action, Option<String>) {
        // 任何非 Tab 键都会中断循环补全状态。
        if key.code != KeyCode::Tab {
            self.tab_completion = None;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);

        if key.code == KeyCode::Esc {
            if self.submitting {
                let _ = self.cancel_tx.send_replace(true);
                return (Action::Continue, None);
            }
            self.editor.clear();
            self.detach_history();
            return (Action::Continue, None);
        }

        if key.code == KeyCode::Char('c') && ctrl {
            if !self.submitting && self.editor.textarea.is_selecting() {
                self.editor.textarea.input(key);
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
                    self.editor.textarea.insert_newline();
                } else if !self.editor.is_blank() {
                    let trimmed = self.editor.text().trim().to_string();
                    if is_exit_command(&trimmed) {
                        return (Action::Quit, None);
                    }
                    self.conv.push_user_message(&trimmed);
                    self.history.push(trimmed.clone());
                    self.editor.clear();
                    self.submitting = true;
                    self.conv_scroll.scroll_to_bottom();
                    return (Action::Submit, Some(trimmed));
                }
            }
            KeyCode::Char('j') if ctrl => self.editor.textarea.insert_newline(),
            KeyCode::Char('u') if ctrl => {
                self.editor.textarea.delete_line_by_head();
            }
            KeyCode::Char('z' | 'Z') if ctrl && shift => {
                self.editor.textarea.redo();
            }
            KeyCode::Char('z' | 'Z') if ctrl => {
                self.editor.textarea.undo();
            }
            KeyCode::Tab => self.complete_command(),
            KeyCode::Backspace if ctrl || alt => {
                self.editor.textarea.delete_word();
            }
            KeyCode::Up | KeyCode::Down if key.modifiers.is_empty() => {
                let before = self.editor.textarea.cursor();
                self.editor.textarea.input(key);
                if self.editor.textarea.cursor() == before {
                    if key.code == KeyCode::Up {
                        self.history_prev();
                    } else {
                        self.history_next();
                    }
                }
            }
            _ => {
                self.editor.textarea.input(key);
            }
        }
        (Action::Continue, None)
    }

    /// Tab 补全内置命令：首个 Tab 计算候选并填入第一项，后续 Tab 循环切换。
    /// 非命令上下文（非 `/` 开头或已输入参数）退化为插入空格。
    fn complete_command(&mut self) {
        if let Some(state) = &mut self.tab_completion {
            state.index = (state.index + 1) % state.matches.len();
            let next = state.matches[state.index];
            self.editor.set_text(next);
            return;
        }
        let text = self.editor.text();
        if !complete::is_command_context(&text) {
            self.editor.textarea.insert_tab();
            return;
        }
        let matches = complete::candidates(&text);
        match matches.len() {
            0 => {}
            1 => self.editor.set_text(matches[0]),
            _ => {
                self.editor.set_text(matches[0]);
                self.tab_completion = Some(TabCompletion { matches, index: 0 });
            }
        }
    }
}
