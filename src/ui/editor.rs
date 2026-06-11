//! 多行文本编辑器组件。
//!
//! 提供多行文本编辑能力：光标移动、文本插入/删除、词级操作、
//! 行级操作以及视口滚动。

use crate::constants;

/// 基于字符（而非字节）的文本长度。
pub(crate) fn char_len(s: &str) -> usize {
    s.chars().count()
}

/// 将字符列位置转换为字节偏移。
pub(crate) fn char_to_byte(s: &str, col: usize) -> usize {
    s.char_indices().nth(col).map(|(b, _)| b).unwrap_or(s.len())
}

pub(crate) struct Editor {
    pub lines: Vec<String>,
    pub row: usize,
    pub col: usize,
    pub scroll_row: usize,
    pub scroll_col: usize,
}

impl Editor {
    pub fn new() -> Self {
        Self {
            lines: vec![String::new()],
            row: 0,
            col: 0,
            scroll_row: 0,
            scroll_col: 0,
        }
    }

    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    pub fn set_text(&mut self, text: &str) {
        self.lines = text.split('\n').map(str::to_string).collect();
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.row = self.lines.len() - 1;
        self.col = char_len(&self.lines[self.row]);
        self.scroll_row = 0;
        self.scroll_col = 0;
    }

    pub fn clear(&mut self) {
        *self = Editor::new();
    }

    pub fn is_blank(&self) -> bool {
        self.lines.iter().all(|l| l.trim().is_empty())
    }

    pub fn at_first_line(&self) -> bool {
        self.row == 0
    }

    pub fn at_last_line(&self) -> bool {
        self.row + 1 == self.lines.len()
    }

    pub fn cur_len(&self) -> usize {
        char_len(&self.lines[self.row])
    }

    pub fn insert_char(&mut self, c: char) {
        if c.is_control() {
            return;
        }
        let byte = char_to_byte(&self.lines[self.row], self.col);
        self.lines[self.row].insert(byte, c);
        self.col += 1;
    }

    pub fn insert_str(&mut self, s: &str) {
        for c in s.chars() {
            match c {
                '\n' => self.newline(),
                '\r' => {}
                '\t' => self.insert_tab(),
                _ => self.insert_char(c),
            }
        }
    }

    /// 插入一个 Tab（展开为 `TAB_WIDTH` 个空格）。
    pub fn insert_tab(&mut self) {
        for _ in 0..constants::TAB_WIDTH {
            self.insert_char(' ');
        }
    }

    pub fn newline(&mut self) {
        let byte = char_to_byte(&self.lines[self.row], self.col);
        let rest = self.lines[self.row].split_off(byte);
        self.lines.insert(self.row + 1, rest);
        self.row += 1;
        self.col = 0;
    }

    pub fn backspace(&mut self) {
        if self.col > 0 {
            let byte = char_to_byte(&self.lines[self.row], self.col - 1);
            self.lines[self.row].remove(byte);
            self.col -= 1;
        } else if self.row > 0 {
            let line = self.lines.remove(self.row);
            self.row -= 1;
            self.col = self.cur_len();
            self.lines[self.row].push_str(&line);
        }
    }

    pub fn delete(&mut self) {
        if self.col < self.cur_len() {
            let byte = char_to_byte(&self.lines[self.row], self.col);
            self.lines[self.row].remove(byte);
        } else if self.row + 1 < self.lines.len() {
            let next = self.lines.remove(self.row + 1);
            self.lines[self.row].push_str(&next);
        }
    }

    pub fn left(&mut self) {
        if self.col > 0 {
            self.col -= 1;
        } else if self.row > 0 {
            self.row -= 1;
            self.col = self.cur_len();
        }
    }

    pub fn right(&mut self) {
        if self.col < self.cur_len() {
            self.col += 1;
        } else if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = 0;
        }
    }

    pub fn up(&mut self) {
        if self.row > 0 {
            self.row -= 1;
            self.col = self.col.min(self.cur_len());
        }
    }

    pub fn down(&mut self) {
        if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = self.col.min(self.cur_len());
        }
    }

    pub fn home(&mut self) {
        self.col = 0;
    }

    pub fn end(&mut self) {
        self.col = self.cur_len();
    }

    pub fn move_word_left(&mut self) {
        if self.col == 0 {
            self.left();
            return;
        }
        let chars: Vec<char> = self.lines[self.row].chars().collect();
        let mut i = self.col;
        while i > 0 && chars[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !chars[i - 1].is_whitespace() {
            i -= 1;
        }
        self.col = i;
    }

    pub fn move_word_right(&mut self) {
        let n = self.cur_len();
        if self.col >= n {
            self.right();
            return;
        }
        let chars: Vec<char> = self.lines[self.row].chars().collect();
        let mut i = self.col;
        while i < n && !chars[i].is_whitespace() {
            i += 1;
        }
        while i < n && chars[i].is_whitespace() {
            i += 1;
        }
        self.col = i;
    }

    pub fn delete_word_left(&mut self) {
        if self.col == 0 {
            self.backspace();
            return;
        }
        let chars: Vec<char> = self.lines[self.row].chars().collect();
        let mut i = self.col;
        while i > 0 && chars[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !chars[i - 1].is_whitespace() {
            i -= 1;
        }
        let kept: String = chars[..i].iter().chain(&chars[self.col..]).collect();
        self.lines[self.row] = kept;
        self.col = i;
    }

    pub fn kill_to_line_start(&mut self) {
        let tail: String = self.lines[self.row].chars().skip(self.col).collect();
        self.lines[self.row] = tail;
        self.col = 0;
    }

    pub fn kill_to_line_end(&mut self) {
        let head: String = self.lines[self.row].chars().take(self.col).collect();
        self.lines[self.row] = head;
    }

    pub fn displayed_rows(&self) -> usize {
        self.lines.len().clamp(1, constants::MAX_TEXT_ROWS)
    }

    pub fn ensure_row_visible(&mut self, rows: usize) {
        if rows == 0 {
            return;
        }
        if self.row < self.scroll_row {
            self.scroll_row = self.row;
        } else if self.row >= self.scroll_row + rows {
            self.scroll_row = self.row + 1 - rows;
        }
    }

    pub fn ensure_col_visible(&mut self, cursor_width: usize, text_width: usize) {
        if text_width == 0 {
            return;
        }
        if cursor_width < self.scroll_col {
            self.scroll_col = cursor_width;
        } else if cursor_width > self.scroll_col + text_width - 1 {
            self.scroll_col = cursor_width + 1 - text_width;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_text() {
        let mut ed = Editor::new();
        ed.insert_str("hello");
        assert_eq!(ed.text(), "hello");
        assert_eq!(ed.col, 5);
    }

    #[test]
    fn newline_splits_line() {
        let mut ed = Editor::new();
        ed.insert_str("ab");
        ed.left();
        ed.newline();
        assert_eq!(ed.text(), "a\nb");
        assert_eq!((ed.row, ed.col), (1, 0));
    }

    #[test]
    fn backspace_merges_lines() {
        let mut ed = Editor::new();
        ed.insert_str("a\nb");
        ed.home();
        ed.backspace();
        assert_eq!(ed.text(), "ab");
        assert_eq!((ed.row, ed.col), (0, 1));
    }

    #[test]
    fn cjk_cursor_width_is_double() {
        let mut ed = Editor::new();
        ed.insert_str("你好");
        assert_eq!(crate::ui::render::prefix_width(&ed.lines[0], ed.col), 4);
    }
}
