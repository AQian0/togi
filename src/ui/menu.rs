//! 居中悬浮菜单（基于 tui-overlay）。
//!
//! Agent 闲置时按 Esc 唤出，Esc 关闭，Enter 执行选中项。
//! 新条目在 [`ACTIONS`] 与 [`MenuAction`] 中各加一行即可。

use crate::ui::style;
use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph};
use tui_overlay::{Backdrop, Overlay, OverlayState};

/// 菜单动作。后续新增条目时在此扩展。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum MenuAction {
    Quit,
}

/// 菜单条目顺序即展示顺序。
const ACTIONS: &[MenuAction] = &[MenuAction::Quit];

const MENU_WIDTH: u16 = 24;

pub(crate) struct Menu {
    state: OverlayState,
    selected: usize,
}

impl Menu {
    pub(crate) fn new() -> Self {
        Self {
            state: OverlayState::new(),
            selected: 0,
        }
    }

    pub(crate) fn is_open(&self) -> bool {
        self.state.is_open()
    }

    pub(crate) fn open(&mut self) {
        self.selected = 0;
        self.state.open();
    }

    pub(crate) fn close(&mut self) {
        self.state.close();
    }

    pub(crate) fn move_up(&mut self) {
        self.selected = self
            .selected
            .checked_sub(1)
            .unwrap_or(ACTIONS.len() - 1);
    }

    pub(crate) fn move_down(&mut self) {
        self.selected = (self.selected + 1) % ACTIONS.len();
    }

    pub(crate) fn selected_action(&self) -> MenuAction {
        ACTIONS[self.selected]
    }

    pub(crate) fn render(&mut self, frame: &mut Frame, area: Rect) {
        let palette = style::c();
        let overlay = Overlay::new()
            .width(Constraint::Length(MENU_WIDTH))
            .height(Constraint::Length(ACTIONS.len() as u16 + 2))
            .backdrop(Backdrop::new(palette.crust))
            .bg(palette.base)
            .block(
                Block::bordered()
                    .border_style(Style::default().fg(palette.surface1))
                    .title(crate::t!("menu-title")),
            );
        frame.render_stateful_widget(overlay, area, &mut self.state);
        let Some(inner) = self.state.inner_area() else {
            return;
        };
        let lines: Vec<Line> = ACTIONS
            .iter()
            .enumerate()
            .map(|(i, action)| {
                let label = match action {
                    MenuAction::Quit => crate::t!("menu-item-quit"),
                };
                let item_style = if i == self.selected {
                    Style::default()
                        .bg(palette.surface1)
                        .fg(palette.text)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().bg(palette.base).fg(palette.text)
                };
                Line::from(format!(" {label} ")).style(item_style)
            })
            .collect();
        frame.render_widget(Paragraph::new(lines), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_wraps_around() {
        let mut menu = Menu::new();
        assert!(!menu.is_open());
        menu.open();
        assert_eq!(menu.selected_action(), ACTIONS[0]);
        menu.move_up();
        assert_eq!(menu.selected_action(), ACTIONS[ACTIONS.len() - 1]);
        for _ in 0..ACTIONS.len() {
            menu.move_down();
        }
        assert_eq!(menu.selected_action(), ACTIONS[ACTIONS.len() - 1]);
        menu.move_down();
        assert_eq!(menu.selected_action(), ACTIONS[0]);
    }
}
