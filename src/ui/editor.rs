//! `ratatui-textarea` 输入编辑器适配层。

use crate::shared::constants;
use crate::ui::style;
use ratatui::style::Style;
use ratatui_textarea::{CursorMove, TextArea, WrapMode};

pub(crate) struct Editor {
    pub(crate) textarea: TextArea<'static>,
}

impl Editor {
    pub fn new() -> Self {
        Self::from_text("")
    }

    fn from_text(text: &str) -> Self {
        let mut textarea = TextArea::from(text.split('\n'));
        textarea.set_tab_length(constants::TAB_WIDTH as u8);
        textarea.set_wrap_mode(WrapMode::Glyph);
        textarea.set_style(style::input_background());
        textarea.set_cursor_line_style(Style::default());
        textarea.set_selection_style(Style::default().bg(style::c().surface1));
        textarea.move_cursor(CursorMove::Jump(u16::MAX, u16::MAX));
        Self { textarea }
    }

    #[must_use]
    pub fn text(&self) -> String {
        self.textarea.lines().join("\n")
    }

    pub fn set_text(&mut self, text: &str) {
        *self = Self::from_text(text);
    }

    pub fn clear(&mut self) {
        *self = Self::new();
    }

    #[must_use]
    pub fn is_blank(&self) -> bool {
        self.textarea
            .lines()
            .iter()
            .all(|line| line.trim().is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_text_moves_cursor_to_the_end() {
        let mut editor = Editor::new();
        editor.set_text("你好\nworld");
        assert_eq!(editor.textarea.cursor(), (1, 5));
    }

    #[test]
    fn clear_resets_undo_history() {
        let mut editor = Editor::new();
        editor.textarea.insert_str("draft");
        editor.clear();
        editor.textarea.undo();
        assert_eq!(editor.text(), "");
    }
}
