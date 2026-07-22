//! 全屏对话 Session。
//!
//! 基于 ratatui 全屏模式，底部始终展示输入区（上横线分隔 + 多行编辑），
//! 对话内容在上方滚动输出。提交后不清除输入区，流式回答实时刷入上方对话区。

use crate::shared::constants;
use crate::ui::OutputItem;
use crate::ui::conversation::Conversation;
use crate::ui::editor::Editor;
use crate::ui::history::History;
use crate::ui::keys::Action;
use crate::ui::render::{self, prefix_width};
use crate::ui::style;
use crate::ui::terminal::{EventPump, TerminalModeGuard};
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{Event, KeyEventKind, MouseEventKind};
use ratatui::{Terminal, TerminalOptions, Viewport};
use std::io::{self, Stdout};
use std::time::Instant;
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

pub struct Session {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    // RAII：Drop 时恢复终端模式，从不读取。
    _terminal_mode: TerminalModeGuard,
    pub(crate) conv: Conversation,
    pub(crate) editor: Editor,
    pub(crate) history: History,
    pub(crate) submitting: bool,
    pub(crate) conv_scroll_offset: usize,
    pub(crate) cancel_tx: watch::Sender<bool>,
    pub(crate) last_ctrl_c: Option<Instant>,
    pub(crate) tab_completion: Option<crate::ui::complete::TabCompletion>,
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
            _terminal_mode: terminal_mode,
            conv: Conversation::new(),
            editor: Editor::new(),
            history: History::load_default(),
            submitting: false,
            conv_scroll_offset: 0,
            cancel_tx,
            last_ctrl_c: None,
            tab_completion: None,
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
        mut on_submit: impl FnMut(String, mpsc::UnboundedSender<OutputItem>),
        cancel_token: CancellationToken,
    ) -> Result<(), crate::ui::UiError> {
        let (out_tx, mut out_rx) = mpsc::unbounded_channel();
        let (markdown_tx, mut markdown_rx) = mpsc::unbounded_channel();

        self.render()?;

        let (event_tx, mut event_rx) = mpsc::unbounded_channel();
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
                        let event = event?;
                        let is_scroll = matches!(&event, Event::Mouse(m) if matches!(m.kind, MouseEventKind::ScrollUp | MouseEventKind::ScrollDown));
                        let is_other_mouse = matches!(&event, Event::Mouse(_));
                        if self.handle_terminal_event(event, &out_tx, &mut on_submit).await {
                            quit = true;
                        }
                        if is_scroll {
                            // 批量消费已排队的滚轮事件，使方向反转即时生效。
                            while let Ok(ev) = event_rx.try_recv() {
                                let Ok(ev) = ev else { break };
                                match &ev {
                                    Event::Mouse(m) if matches!(m.kind, MouseEventKind::ScrollUp | MouseEventKind::ScrollDown) => {
                                        if self.handle_terminal_event(ev, &out_tx, &mut on_submit).await {
                                            quit = true;
                                            break;
                                        }
                                    }
                                    _ => {
                                        // 遇到非滚轮事件，立即处理然后停止排空，
                                        // 确保键盘事件不丢失即时渲染。
                                        if self.handle_terminal_event(ev, &out_tx, &mut on_submit).await {
                                            quit = true;
                                        }
                                        break;
                                    }
                                }
                            }
                        }
                        if is_other_mouse && !is_scroll {
                            // 忽略非滚轮鼠标事件，不触发渲染。
                        } else {
                            self.render()?;
                            last_render = Instant::now();
                            render_pending = false;
                        }
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
        out_tx: &mpsc::UnboundedSender<OutputItem>,
        on_submit: &mut impl FnMut(String, mpsc::UnboundedSender<OutputItem>),
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
            Event::Mouse(mouse) => match mouse.kind {
                MouseEventKind::ScrollUp => {
                    self.conv_scroll_offset = self.conv_scroll_offset.saturating_add(1);
                }
                MouseEventKind::ScrollDown => {
                    self.conv_scroll_offset = self.conv_scroll_offset.saturating_sub(1);
                }
                _ => {}
            },
            Event::Paste(data) if !self.submitting => {
                self.detach_history();
                self.editor.insert_str(&data);
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

    pub(crate) fn conv_page_height(&self) -> usize {
        let term_h = self.terminal.size().map(|s| s.height).unwrap_or(24);
        if term_h < 5 {
            return 0;
        }
        let input_rows = self.editor.displayed_rows() as u16;
        let input_height = (input_rows + 2).min(term_h.saturating_sub(2));
        let conv_height = term_h.saturating_sub(input_height);
        conv_height.saturating_sub(2) as usize
    }

    pub(crate) fn apply_output(&mut self, item: OutputItem) -> bool {
        let done = self.conv.apply_output(item);
        if done {
            self.submitting = false;
        }
        done
    }

    fn render(&mut self) -> io::Result<()> {
        let (term_w, term_h) = {
            let size = self.terminal.size()?;
            (size.width, size.height)
        };
        if term_h < constants::MIN_TERMINAL_HEIGHT {
            return Ok(());
        }

        let input_rows = self.editor.displayed_rows() as u16;
        let input_height = (input_rows + 2).min(term_h.saturating_sub(2));
        let visible_rows = input_height.saturating_sub(2) as usize;
        let text_width = term_w as usize;

        self.editor.ensure_row_visible(visible_rows);
        let cursor_line_len = prefix_width(&self.editor.lines[self.editor.row], self.editor.col);
        self.editor.ensure_col_visible(cursor_line_len, text_width);

        let display_lines = self.conv.cached_display_lines(term_w);
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
                    display_lines,
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

    pub(crate) fn detach_history(&mut self) {
        self.history.detach();
    }

    pub(crate) fn history_prev(&mut self) {
        if let Some(entry) = self.history.previous(self.editor.text()) {
            self.editor.set_text(&entry);
        }
    }

    pub(crate) fn history_next(&mut self) {
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
