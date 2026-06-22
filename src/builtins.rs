use crate::ui::interaction::OutputItem;
use rig::message::Message;
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc};

/// 帮助命令的描述列表。命令名本身不翻译（是用户输入的关键字），
/// 仅翻译右侧描述文本。
static HELP_ROWS: &[(&str, &str)] = &[
    ("/help", "builtins-help-desc"),
    ("/clear", "builtins-clear-desc"),
    ("/cwd", "builtins-cwd-desc"),
    (
        "/exit、/quit",
        "builtins-exit-desc",
    ),
];

pub async fn handle_command(
    line: &str,
    tx: mpsc::UnboundedSender<OutputItem>,
    history: &Arc<RwLock<Arc<[Message]>>>,
) -> bool {
    match line {
        "/help" => {
            send_notice(&tx, "");
            send_notice(&tx, &crate::t!("builtins-help-title"));
            for (cmd, desc_key) in HELP_ROWS {
                send_notice(&tx, &format!("  {cmd:<14}{}", crate::t!(desc_key)));
            }
            send_notice(&tx, "");
            send_notice(&tx, &crate::t!("builtins-shortcuts-title"));
            send_notice(&tx, &crate::t!("builtins-shortcut-esc"));
            send_notice(&tx, &crate::t!("builtins-shortcut-ctrl-c"));
            send_notice(&tx, &crate::t!("builtins-shortcut-page"));
            send_notice(&tx, "");
        }
        "/clear" => {
            let mut guard = history.write().await;
            let count = guard.len();
            *guard = Arc::from(Vec::new());
            send_notice(&tx, &crate::t!("builtins-clear-done", count = count));
        }
        "/cwd" => {
            let cwd = std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "?".to_string());
            send_notice(&tx, &crate::t!("builtins-cwd-display", cwd = cwd));
        }
        other => {
            send_notice(
                &tx,
                &crate::t!("builtins-unknown-command", command = other),
            );
        }
    }
    tx.send(OutputItem::Done).is_ok()
}

fn send_notice(tx: &mpsc::UnboundedSender<OutputItem>, msg: &str) {
    if tx.send(OutputItem::Notice(msg.to_string())).is_err() {
        // 接收端已关闭，后续 send 也会失败，直接忽略。
    }
}
