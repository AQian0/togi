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
            let page = self.conv_page_height().max(1);
            if key.code == KeyCode::PageUp {
                self.conv_scroll_offset = self.conv_scroll_offset.saturating_add(page);
            } else {
                self.conv_scroll_offset = self.conv_scroll_offset.saturating_sub(page);
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
                    self.editor.newline();
                } else if !self.editor.is_blank() {
                    let trimmed = self.editor.text().trim().to_string();
                    if is_exit_command(&trimmed) {
                        return (Action::Quit, None);
                    }
                    self.conv.push_user_message(&trimmed);
                    self.history.push(trimmed.clone());
                    self.editor.clear();
                    self.submitting = true;
                    self.conv_scroll_offset = 0;
                    return (Action::Submit, Some(trimmed));
                }
            }
            KeyCode::Char('j') if ctrl => self.editor.newline(),
            KeyCode::Char('a') if ctrl => self.editor.home(),
            KeyCode::Char('e') if ctrl => self.editor.end(),
            KeyCode::Char('u') if ctrl => self.editor.kill_to_line_start(),
            KeyCode::Char('k') if ctrl => self.editor.kill_to_line_end(),
            KeyCode::Char('w') if ctrl => self.editor.delete_word_left(),
            KeyCode::Char(_) if alt => {}
            KeyCode::Char(c) => self.editor.insert_char(c),
            KeyCode::Tab => self.complete_command(),
            KeyCode::Backspace => {
                if ctrl || alt {
                    self.editor.delete_word_left();
                } else {
                    self.editor.backspace();
                }
            }
            KeyCode::Delete => self.editor.delete(),
            KeyCode::Left => {
                if ctrl || alt {
                    self.editor.move_word_left();
                } else {
                    self.editor.left();
                }
            }
            KeyCode::Right => {
                if ctrl || alt {
                    self.editor.move_word_right();
                } else {
                    self.editor.right();
                }
            }
            KeyCode::Up => {
                if self.editor.at_first_line() {
                    self.history_prev();
                } else {
                    self.editor.up();
                }
            }
            KeyCode::Down => {
                if self.editor.at_last_line() {
                    self.history_next();
                } else {
                    self.editor.down();
                }
            }
            KeyCode::Home => self.editor.home(),
            KeyCode::End => self.editor.end(),
            _ => {}
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
            self.editor.insert_tab();
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
