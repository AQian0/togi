//! 终端模式管理与事件轮询。
//!
//! - [`TerminalModeGuard`]：管理 raw mode、alternate screen、bracketed paste 和
//!   鼠标捕获的进入与退出（RAII）。
//! - [`EventPump`]：后台线程轮询 crossterm 事件并通过 channel 发送。
use crate::shared::constants;
use ratatui::crossterm::event::{self, Event};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use std::io;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub(crate) struct TerminalModeGuard {
    active: bool,
}

impl TerminalModeGuard {
    pub(crate) fn activate() -> io::Result<Self> {
        enable_raw_mode()?;
        if let Err(err) = execute!(
            io::stdout(),
            EnterAlternateScreen,
            ratatui::crossterm::event::EnableBracketedPaste,
            ratatui::crossterm::event::EnableMouseCapture
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
        let _ = execute!(
            io::stdout(),
            ratatui::crossterm::event::DisableBracketedPaste
        );
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

pub(crate) struct EventPump {
    cancel: CancellationToken,
    handle: tokio::task::JoinHandle<()>,
}

impl EventPump {
    pub(crate) fn start(tx: tokio::sync::mpsc::UnboundedSender<io::Result<Event>>) -> Self {
        let cancel = CancellationToken::new();
        let cancel_worker = cancel.clone();
        let handle = tokio::task::spawn_blocking(move || {
            while !cancel_worker.is_cancelled() {
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
        Self { cancel, handle }
    }

    pub(crate) async fn stop(self) {
        self.cancel.cancel();
        let _ = self.handle.await;
    }
}
