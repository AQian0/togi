//! 全屏对话 Session。
//!
//! 基于 ratatui 全屏模式，底部始终展示输入区（上横线分隔 + 多行编辑），
//! 对话内容在上方滚动输出。提交后不清除输入区，流式回答实时刷入上方对话区。

use crate::pipeline::confirm::ApprovalPolicy;
use crate::shared::constants;
use crate::tools::ApprovalDecision;
use crate::ui::OutputItem;
use crate::ui::conversation::Conversation;
use crate::ui::editor::Editor;
use crate::ui::history::History;
use crate::ui::keys::Action;
use crate::ui::menu::Menu;
use crate::ui::render;
use crate::ui::style;
use crate::ui::terminal::{EventPump, TerminalModeGuard};
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{Event, KeyEventKind, MouseEventKind};
use ratatui::{Terminal, TerminalOptions, Viewport};
use std::collections::VecDeque;
use std::io::{self, Stdout};
use std::time::Instant;
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;
use tui_widgets::scrollview::ScrollViewState;

struct PendingApproval {
    name: String,
    summary: String,
    depth: u32,
    response: oneshot::Sender<ApprovalDecision>,
}

pub struct Session {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    // RAII：Drop 时恢复终端模式，从不读取。
    _terminal_mode: TerminalModeGuard,
    pub(crate) conv: Conversation,
    pub(crate) editor: Editor,
    pub(crate) history: History,
    pub(crate) submitting: bool,
    pub(crate) conv_scroll: ScrollViewState,
    pub(crate) cancel_tx: watch::Sender<bool>,
    pub(crate) last_ctrl_c: Option<Instant>,
    pub(crate) menu: Menu,
    pub(crate) tab_completion: Option<crate::ui::complete::TabCompletion>,
    pending_approvals: VecDeque<PendingApproval>,
    approval_policy: ApprovalPolicy,
}

impl Session {
    pub fn new(approval_policy: ApprovalPolicy) -> Result<Self, crate::ui::UiError> {
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
            conv_scroll: ScrollViewState::new(),
            cancel_tx,
            last_ctrl_c: None,
            menu: Menu::new(),
            tab_completion: None,
            pending_approvals: VecDeque::new(),
            approval_policy,
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
                    self.conv_scroll.scroll_up();
                }
                MouseEventKind::ScrollDown => {
                    self.conv_scroll.scroll_down();
                }
                _ => {}
            },
            Event::Paste(data) if !self.submitting => {
                self.detach_history();
                self.editor.textarea.insert_str(&data);
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
            self.editor.textarea.insert_str(&text);
        }
    }

    pub(crate) fn apply_output(&mut self, item: OutputItem) -> bool {
        self.advance_closed_approvals();
        let done = match item {
            OutputItem::Approval {
                name,
                summary,
                depth,
                response,
            } => {
                if self.approval_policy.is_allowed(&name) {
                    let _ = response.send(ApprovalDecision::AlwaysAllow);
                    false
                } else {
                    let show = self.pending_approvals.is_empty();
                    self.pending_approvals.push_back(PendingApproval {
                        name,
                        summary,
                        depth,
                        response,
                    });
                    if show {
                        self.show_next_approval();
                    }
                    false
                }
            }
            OutputItem::Done => {
                self.pending_approvals.clear();
                self.conv.apply_output(OutputItem::Done)
            }
            item => self.conv.apply_output(item),
        };
        if done {
            self.submitting = false;
        }
        done
    }

    pub(crate) fn has_pending_approval(&self) -> bool {
        !self.pending_approvals.is_empty()
    }

    pub(crate) fn resolve_approval(&mut self, decision: ApprovalDecision) {
        let Some(current) = self.pending_approvals.pop_front() else {
            return;
        };
        if current.response.send(decision).is_err() {
            self.show_next_approval();
            return;
        }
        if decision == ApprovalDecision::AlwaysAllow {
            self.approval_policy.allow(&current.name);
        }
        self.conv
            .push_approval_result(&current.name, current.depth, decision);
        self.show_next_approval();
    }

    fn advance_closed_approvals(&mut self) {
        let mut advanced = false;
        while self
            .pending_approvals
            .front()
            .is_some_and(|pending| pending.response.is_closed())
        {
            self.pending_approvals.pop_front();
            advanced = true;
        }
        if advanced {
            self.show_next_approval();
        }
    }

    fn show_next_approval(&mut self) {
        loop {
            let Some(pending) = self.pending_approvals.front() else {
                return;
            };
            if self.approval_policy.is_allowed(&pending.name) || pending.response.is_closed() {
                let Some(pending) = self.pending_approvals.pop_front() else {
                    return;
                };
                if self.approval_policy.is_allowed(&pending.name) {
                    let _ = pending.response.send(ApprovalDecision::AlwaysAllow);
                }
                continue;
            }
            self.conv
                .push_approval(&pending.name, &pending.summary, pending.depth);
            return;
        }
    }

    fn render(&mut self) -> io::Result<()> {
        let (term_w, term_h) = {
            let size = self.terminal.size()?;
            (size.width, size.height)
        };
        if term_h < constants::MIN_TERMINAL_HEIGHT {
            return Ok(());
        }

        let scroll_view = self.conv.cached_scroll_view(term_w, self.submitting);
        let editor = &self.editor;
        let conv_scroll = &mut self.conv_scroll;
        let menu = &mut self.menu;

        self.terminal.draw(|frame| {
            render::render_frame(
                frame,
                render::FrameRenderState {
                    scroll_view,
                    conv_state: conv_scroll,
                    editor,
                    menu,
                    separator_style: style::separator(),
                    dim_style: style::dim(),
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
