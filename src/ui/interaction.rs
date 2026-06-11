//! Pi 风格的持久化全屏 UI。
//!
//! 基于 ratatui 全屏模式，底部始终展示输入区（上横线分隔 + 多行编辑），
//! 对话内容在上方滚动输出。提交后不清除输入区，流式回答实时刷入上方对话区。
use crate::constants;
use crate::ui::conversation::Conversation;
use crate::ui::editor::Editor;
use crate::ui::history::History;
use crate::ui::render::{self, prefix_width};
use crate::ui::style;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, Event, KeyCode,
    KeyEvent, KeyEventKind, KeyModifiers,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::{Terminal, TerminalOptions, Viewport};
use std::io::{self, Stdout};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

pub use crate::ui::output::{OutputItem, SectionKind};

struct TerminalModeGuard {
    active: bool,
}

impl TerminalModeGuard {
    fn activate() -> io::Result<Self> {
        enable_raw_mode()?;
        if let Err(err) = execute!(
            io::stdout(),
            EnterAlternateScreen,
            EnableBracketedPaste,
            DisableMouseCapture
        ) {
            let _ = disable_raw_mode();
            return Err(err);
        }
        Ok(Self { active: true })
    }

    fn restore(&mut self) {
        if !self.active {
            return;
        }
        let _ = execute!(io::stdout(), DisableBracketedPaste);
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        let _ = disable_raw_mode();
        self.active = false;
    }
}

impl Drop for TerminalModeGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

struct EventPump {
    stop: Arc<AtomicBool>,
    handle: tokio::task::JoinHandle<()>,
}

impl EventPump {
    fn start(tx: tokio::sync::mpsc::UnboundedSender<io::Result<Event>>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_worker = Arc::clone(&stop);
        let handle = tokio::task::spawn_blocking(move || {
            while !stop_worker.load(Ordering::Relaxed) {
                match event::poll(Duration::from_millis(constants::POLL_INTERVAL_MS)) {
                    Ok(false) => {}
                    Ok(true) => match event::read() {
                        Ok(event) => {
                            if tx.send(Ok(event)).is_err() {
                                break;
                            }
                        }
                        Err(err) => {
                            let _ = tx.send(Err(err));
                            break;
                        }
                    },
                    Err(err) => {
                        let _ = tx.send(Err(err));
                        break;
                    }
                }
            }
        });
        Self { stop, handle }
    }

    async fn stop(self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.handle.await;
    }
}

pub struct Session {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    terminal_mode: TerminalModeGuard,
    conv: Conversation,
    editor: Editor,
    history: History,
    submitting: bool,
    conv_scroll_offset: usize,
    cancel_tx: watch::Sender<bool>,
    last_ctrl_c: Option<Instant>,
}

impl Session {
    pub fn new() -> Result<Self, crate::ui::UiError> {
        let terminal_mode = TerminalModeGuard::activate()?;
        let terminal = Terminal::with_options(
            CrosstermBackend::new(io::stdout()),
            TerminalOptions {
                viewport: Viewport::Fullscreen,
            },
        )?;
        let (cancel_tx, _) = watch::channel(false);
        Ok(Self {
            terminal,
            terminal_mode,
            conv: Conversation::new(),
            editor: Editor::new(),
            history: History::load_default(),
            submitting: false,
            conv_scroll_offset: 0,
            cancel_tx,
            last_ctrl_c: None,
        })
    }

    pub fn cancel_sender(&self) -> watch::Sender<bool> {
        self.cancel_tx.clone()
    }

    /// 运行主循环。
    ///
    /// `cancel_token` 用于响应外部取消信号（如 Ctrl-C），提前退出循环。
    pub async fn run(
        &mut self,
        on_submit: impl FnMut(String, tokio::sync::mpsc::UnboundedSender<OutputItem>),
        cancel_token: CancellationToken,
    ) -> Result<(), crate::ui::UiError> {
        self.run_inner(on_submit, cancel_token).await
    }

    async fn run_inner(
        &mut self,
        mut on_submit: impl FnMut(String, tokio::sync::mpsc::UnboundedSender<OutputItem>),
        cancel_token: CancellationToken,
    ) -> Result<(), crate::ui::UiError> {
        let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel();
        let (markdown_tx, mut markdown_rx) = tokio::sync::mpsc::unbounded_channel();

        self.render()?;

        let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
        let event_pump = EventPump::start(event_tx);
        let mut last_render = Instant::now();
        let mut render_pending = false;
        let mut quit = false;

        let result: Result<(), crate::ui::UiError> = async {
            while !quit {
                tokio::select! {
                    _ = cancel_token.cancelled() => break,
                    maybe_event = event_rx.recv() => {
                        let Some(event) = maybe_event else {
                            break;
                        };
                        if self.handle_terminal_event(event?, &out_tx, &mut on_submit).await {
                            quit = true;
                        }
                        self.render()?;
                        last_render = Instant::now();
                        render_pending = false;
                    }
                    maybe_item = out_rx.recv() => {
                        let Some(item) = maybe_item else {
                            continue;
                        };
                        let mut force_render = self.apply_output(item);
                        while let Ok(item) = out_rx.try_recv() {
                            force_render |= self.apply_output(item);
                        }
                        self.conv.schedule_live_markdown_render(&markdown_tx);
                        render_pending = true;
                        if force_render || last_render.elapsed() >= constants::OUTPUT_RENDER_INTERVAL {
                            self.render()?;
                            last_render = Instant::now();
                            render_pending = false;
                        }
                    }
                    maybe_rendered = markdown_rx.recv() => {
                        let Some(rendered) = maybe_rendered else {
                            continue;
                        };
                        if self.conv.apply_markdown_render(rendered) {
                            render_pending = true;
                        }
                        self.conv.schedule_live_markdown_render(&markdown_tx);
                    }
                    _ = tokio::time::sleep(constants::OUTPUT_RENDER_INTERVAL.saturating_sub(last_render.elapsed())), if render_pending => {
                        self.render()?;
                        last_render = Instant::now();
                        render_pending = false;
                    }
                }
            }
            Ok(())
        }
        .await;

        event_pump.stop().await;
        result
    }

    async fn handle_terminal_event(
        &mut self,
        event: Event,
        out_tx: &tokio::sync::mpsc::UnboundedSender<OutputItem>,
        on_submit: &mut impl FnMut(String, tokio::sync::mpsc::UnboundedSender<OutputItem>),
    ) -> bool {
        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                let (action, message) = self.handle_key(key);
                match action {
                    Action::Continue => {}
                    Action::Submit => {
                        let msg = message.unwrap();
                        on_submit(msg, out_tx.clone());
                    }
                    Action::Quit => return true,
                    Action::Paste => self.paste_from_clipboard().await,
                }
            }
            Event::Mouse(_) => {}
            Event::Paste(data) => {
                if !self.submitting {
                    self.detach_history();
                    self.editor.insert_str(&data);
                }
            }
            Event::Resize(_, _) => {}
            _ => {}
        }
        false
    }

    async fn paste_from_clipboard(&mut self) {
        if self.submitting {
            return;
        }
        self.detach_history();
        let text = tokio::task::spawn_blocking(|| {
            arboard::Clipboard::new()
                .ok()
                .and_then(|mut c| c.get_text().ok())
        })
        .await
        .ok()
        .flatten();
        if let Some(text) = text {
            self.editor.insert_str(&text);
        }
    }

    fn conv_page_height(&self) -> usize {
        let term_h = self.terminal.size().map(|s| s.height).unwrap_or(24);
        if term_h < 5 {
            return 0;
        }
        let input_rows = self.editor.displayed_rows() as u16;
        let input_height = (input_rows + 2).min(term_h.saturating_sub(2));
        let conv_height = term_h.saturating_sub(input_height);
        conv_height.saturating_sub(2) as usize
    }

    fn handle_key(&mut self, key: KeyEvent) -> (Action, Option<String>) {
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
            KeyCode::Tab => self.editor.insert_tab(),
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

    fn apply_output(&mut self, item: OutputItem) -> bool {
        let done = self.conv.apply_output(item);
        if done {
            self.submitting = false;
        }
        done
    }

    fn render(&mut self) -> io::Result<()> {
        let (_, term_h) = {
            let size = self.terminal.size()?;
            (size.width, size.height)
        };
        if term_h < constants::MIN_TERMINAL_HEIGHT {
            return Ok(());
        }

        let input_rows = self.editor.displayed_rows() as u16;
        let input_height = (input_rows + 2).min(term_h.saturating_sub(2));
        let visible_rows = input_height.saturating_sub(2) as usize;
        let text_width = self.terminal.size()?.width as usize;

        self.editor.ensure_row_visible(visible_rows);
        let cursor_line_len = prefix_width(&self.editor.lines[self.editor.row], self.editor.col);
        self.editor.ensure_col_visible(cursor_line_len, text_width);

        let conv_lines = self.conv.all_lines_with_align();
        let submitting = self.submitting;
        let editor_lines = self.editor.lines.clone();
        let editor_row = self.editor.row;
        let editor_col = self.editor.col;
        let editor_scroll_row = self.editor.scroll_row;
        let editor_scroll_col = self.editor.scroll_col;
        let conv_scroll = self.conv_scroll_offset;

        self.terminal.draw(|frame| {
            render::render_frame(
                frame,
                render::FrameRenderState {
                    conv_lines: &conv_lines,
                    submitting,
                    conv_scroll_offset: conv_scroll,
                    editor_lines: &editor_lines,
                    editor_row,
                    editor_col,
                    editor_scroll_row,
                    editor_scroll_col,
                    visible_rows,
                    text_width,
                    separator_style: style::separator(),
                    dim_style: style::dim(),
                    normal_style: style::normal(),
                },
            );
        })?;
        Ok(())
    }

    fn detach_history(&mut self) {
        self.history.detach();
    }

    fn history_prev(&mut self) {
        if let Some(entry) = self.history.previous(self.editor.text()) {
            self.editor.set_text(&entry);
        }
    }

    fn history_next(&mut self) {
        if let Some(entry) = self.history.next() {
            self.editor.set_text(&entry);
        }
    }

    pub fn save_history(&mut self) -> Result<(), crate::ui::UiError> {
        self.history
            .save()
            .map_err(|source| crate::ui::UiError::HistorySave { source })
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.terminal_mode.restore();
    }
}

enum Action {
    Continue,
    Submit,
    Quit,
    Paste,
}

fn is_exit_command(input: &str) -> bool {
    matches!(input, "exit" | "quit" | "退出" | "/exit" | "/quit")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_commands_recognised() {
        assert!(is_exit_command("exit"));
        assert!(is_exit_command("退出"));
        assert!(!is_exit_command("hello"));
    }
}
