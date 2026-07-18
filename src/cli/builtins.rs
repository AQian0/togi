use crate::shared::error::TogiError;
use crate::store::MessageStore;
use crate::ui::interaction::OutputItem;
use rig::message::Message;
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc};

/// 帮助命令的描述列表。命令名本身不翻译（是用户输入的关键字），
/// 仅翻译右侧描述文本。
static HELP_ROWS: &[(&str, &str)] = &[
    ("/help", "builtins-help-desc"),
    ("/clear", "builtins-clear-desc"),
    ("/history", "builtins-history-desc"),
    ("/cwd", "builtins-cwd-desc"),
    ("/exit、/quit", "builtins-exit-desc"),
];

pub async fn handle_command(
    line: &str,
    tx: mpsc::UnboundedSender<OutputItem>,
    history: &Arc<RwLock<Arc<[Message]>>>,
    store: Option<&Arc<dyn MessageStore>>,
    session_id: &str,
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
            drop(guard);
            if let Some(store) = store {
                if let Err(err) = store.clear(session_id).await {
                    send_notice(
                        &tx,
                        &crate::t!("store-clear-error", error = err.user_message()),
                    );
                }
            }
            send_notice(&tx, &crate::t!("builtins-clear-done", count = count));
        }
        "/history" => {
            let guard = history.read().await;
            let total = guard.len();
            if total == 0 {
                send_notice(&tx, &crate::t!("builtins-history-empty"));
            } else {
                send_notice(&tx, "");
                send_notice(&tx, &crate::t!("builtins-history-title", count = total));
                for (i, msg) in guard.iter().enumerate() {
                    let (role, preview) = message_summary(msg);
                    send_notice(
                        &tx,
                        &format!("  {:>3}  {:<10}{}", i + 1, role, preview),
                    );
                }
                send_notice(&tx, "");
            }
        }
        "/cwd" => {
            let cwd = std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "?".to_string());
            send_notice(&tx, &crate::t!("builtins-cwd-display", cwd = cwd));
        }
        other => {
            send_notice(&tx, &crate::t!("builtins-unknown-command", command = other));
        }
    }
    tx.send(OutputItem::Done).is_ok()
}

/// 提取消息的角色标签和首行文本摘要（最多 72 字符）。
fn message_summary(msg: &Message) -> (&'static str, String) {
    use rig::message::{AssistantContent, UserContent};
    match msg {
        Message::System { content } => ("system", truncate_preview(content)),
        Message::User { content } => {
            for item in content.iter() {
                match item {
                    UserContent::Text(t) => return ("user", truncate_preview(&t.text)),
                    UserContent::ToolResult(_) => {
                        return ("user", "[tool result]".to_string());
                    }
                    _ => {}
                }
            }
            ("user", String::new())
        }
        Message::Assistant { content, .. } => {
            for item in content.iter() {
                match item {
                    AssistantContent::Text(t) => {
                        return ("assistant", truncate_preview(&t.text));
                    }
                    AssistantContent::ToolCall(tc) => {
                        return ("assistant", format!("[tool: {}]", tc.function.name));
                    }
                    AssistantContent::Reasoning(r) => {
                        let text = r.display_text();
                        return ("assistant", truncate_preview(&text));
                    }
                    _ => {}
                }
            }
            ("assistant", String::new())
        }
    }
}

/// 截取首行文本，超过 `max` 字符时截断并加省略号。
fn truncate_preview(text: &str) -> String {
    const MAX: usize = 72;
    let first_line = text.lines().next().unwrap_or(text);
    if first_line.chars().count() <= MAX {
        first_line.to_string()
    } else {
        let truncated: String = first_line.chars().take(MAX - 1).collect();
        format!("{truncated}\u{2026}")
    }
}

fn send_notice(tx: &mpsc::UnboundedSender<OutputItem>, msg: &str) {
    if tx.send(OutputItem::Notice(msg.to_string())).is_err() {
        // 接收端已关闭，后续 send 也会失败，直接忽略。
    }
}
