//! `ratatui-textarea` 输入框构造。

use crate::shared::constants;
use crate::ui::style;
use ratatui::style::Style;
use ratatui_textarea::{CursorMove, TextArea, WrapMode};

/// 构造带应用样式的输入框（光标跳到文本末尾）。
pub(crate) fn make_textarea(text: &str) -> TextArea<'static> {
    let mut textarea = TextArea::from(text.split('\n'));
    textarea.set_tab_length(constants::TAB_WIDTH as u8);
    textarea.set_wrap_mode(WrapMode::Glyph);
    textarea.set_style(style::input_background());
    textarea.set_cursor_line_style(Style::default());
    textarea.set_selection_style(Style::default().bg(style::c().surface1));
    textarea.move_cursor(CursorMove::Jump(u16::MAX, u16::MAX));
    textarea
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn make_textarea_moves_cursor_to_the_end() {
        let textarea = make_textarea("你好\nworld");
        assert_eq!(textarea.cursor(), (1, 5));
    }

    #[test]
    fn rebuild_resets_undo_history() {
        let mut textarea = make_textarea("");
        textarea.insert_str("draft");
        let mut textarea = make_textarea("");
        textarea.undo();
        assert!(textarea.lines().iter().all(|l| l.is_empty()));
    }
}
