use crate::context::ContextCheckpoint;
use crate::shared::error::TogiError;
use crate::store::HistoryStore;
use crate::ui::OutputItem;
use rig::message::Message;
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc};

/// 帮助命令的描述列表。命令名本身不翻译（是用户输入的关键字），
/// 仅翻译右侧描述文本。
pub(crate) static HELP_ROWS: &[(&str, &str)] = &[
    ("/help", "builtins-help-desc"),
    ("/clear", "builtins-clear-desc"),
    ("/history", "builtins-history-desc"),
    ("/sessions", "builtins-sessions-desc"),
    ("/switch", "builtins-switch-desc"),
    ("/new", "builtins-new-desc"),
    ("/delete", "builtins-delete-desc"),
    ("/cwd", "builtins-cwd-desc"),
    ("/index", "builtins-index-desc"),
    ("/exit、/quit", "builtins-exit-desc"),
];

pub async fn handle_command(
    line: &str,
    tx: mpsc::UnboundedSender<OutputItem>,
    history: &Arc<RwLock<Arc<[Message]>>>,
    store: Option<&Arc<HistoryStore>>,
    session_id: &Arc<RwLock<String>>,
    context: &Arc<RwLock<ContextCheckpoint>>,
) -> bool {
    // 先拆分命令和参数
    let mut parts = line.splitn(2, ' ');
    let cmd = parts.next().unwrap_or("");
    let arg = parts.next().unwrap_or("").trim();

    match cmd {
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
            *context.write().await = ContextCheckpoint::default();
            let sid = session_id.read().await.clone();
            if let Some(store) = store {
                if let Err(err) = store.clear(&sid).await {
                    send_notice(
                        &tx,
                        &crate::t!("store-clear-error", error = err.user_message()),
                    );
                }
                if let Err(err) = store.clear_context(&sid).await {
                    send_notice(
                        &tx,
                        &crate::t!("context-save-error", error = err.user_message()),
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
                    send_notice(&tx, &format!("  {:>3}  {:<10}{}", i + 1, role, preview));
                }
                send_notice(&tx, "");
            }
        }
        "/sessions" => {
            let Some(store) = store else {
                send_notice(&tx, &crate::t!("builtins-sessions-unavailable"));
                return true;
            };
            match store.list_sessions().await {
                Ok(sessions) => {
                    if sessions.is_empty() {
                        send_notice(&tx, &crate::t!("builtins-sessions-empty"));
                    } else {
                        let current_sid = session_id.read().await.clone();
                        send_notice(&tx, "");
                        send_notice(
                            &tx,
                            &crate::t!("builtins-sessions-title", count = sessions.len()),
                        );
                        for (i, s) in sessions.iter().enumerate() {
                            let marker = if s.id == *current_sid { " *" } else { "" };
                            let title = if s.title.is_empty() { &s.id } else { &s.title };
                            send_notice(
                                &tx,
                                &format!(
                                    "  {:>3}  {:<36}  {:<20}  {}{}",
                                    i + 1,
                                    s.id,
                                    s.updated_at,
                                    truncate_preview(title, 24),
                                    marker
                                ),
                            );
                        }
                        send_notice(&tx, "");
                    }
                }
                Err(err) => {
                    send_notice(
                        &tx,
                        &crate::t!("store-query-error", error = err.user_message()),
                    );
                }
            }
        }
        "/switch" => {
            let Some(store) = store else {
                send_notice(&tx, &crate::t!("builtins-sessions-unavailable"));
                return true;
            };
            if arg.is_empty() {
                send_notice(&tx, &crate::t!("builtins-switch-usage"));
                return true;
            }
            match resolve_session_id(store, arg).await {
                Ok(target_sid) => {
                    let current_sid = session_id.read().await.clone();
                    if target_sid == *current_sid {
                        send_notice(&tx, &crate::t!("builtins-switch-same"));
                        return true;
                    }
                    // 加载目标会话历史
                    match store.load(&target_sid).await {
                        Ok(messages) => {
                            let count = messages.len();
                            *history.write().await = Arc::from(messages);
                            *session_id.write().await = target_sid.clone();
                            match store.load_context(&target_sid).await {
                                Ok(checkpoint) => {
                                    *context.write().await = checkpoint;
                                }
                                Err(err) => {
                                    *context.write().await = ContextCheckpoint::default();
                                    send_notice(
                                        &tx,
                                        &crate::t!(
                                            "context-load-error",
                                            error = err.user_message()
                                        ),
                                    );
                                }
                            }
                            send_notice(
                                &tx,
                                &crate::t!("builtins-switch-done", id = target_sid, count = count),
                            );
                        }
                        Err(err) => {
                            send_notice(
                                &tx,
                                &crate::t!("store-load-error", error = err.user_message()),
                            );
                        }
                    }
                }
                Err(msg) => {
                    send_notice(&tx, &msg);
                }
            }
        }
        "/new" => {
            let Some(store) = store else {
                send_notice(&tx, &crate::t!("builtins-sessions-unavailable"));
                return true;
            };
            let title = if arg.is_empty() {
                crate::t!("builtins-new-default-title")
            } else {
                arg.to_string()
            };
            match store.create_session(&title).await {
                Ok(new_sid) => {
                    *history.write().await = Arc::from(Vec::new());
                    *session_id.write().await = new_sid.clone();
                    *context.write().await = ContextCheckpoint::default();
                    send_notice(
                        &tx,
                        &crate::t!("builtins-new-done", id = new_sid, title = title),
                    );
                }
                Err(err) => {
                    send_notice(
                        &tx,
                        &crate::t!("store-query-error", error = err.user_message()),
                    );
                }
            }
        }
        "/delete" => {
            let Some(store) = store else {
                send_notice(&tx, &crate::t!("builtins-sessions-unavailable"));
                return true;
            };
            if arg.is_empty() {
                send_notice(&tx, &crate::t!("builtins-delete-usage"));
                return true;
            }
            match resolve_session_id(store, arg).await {
                Ok(target_sid) => {
                    let current_sid = session_id.read().await.clone();
                    if target_sid == *current_sid {
                        send_notice(&tx, &crate::t!("builtins-delete-current"));
                        return true;
                    }
                    match store.delete_session(&target_sid).await {
                        Ok(()) => {
                            send_notice(&tx, &crate::t!("builtins-delete-done", id = target_sid));
                        }
                        Err(err) => {
                            send_notice(
                                &tx,
                                &crate::t!("store-query-error", error = err.user_message()),
                            );
                        }
                    }
                }
                Err(msg) => {
                    send_notice(&tx, &msg);
                }
            }
        }
        "/index" => {
            let Some(store) = store else {
                send_notice(&tx, &crate::t!("builtins-index-unavailable"));
                return true;
            };
            let cwd = std::env::current_dir().unwrap_or_default();
            let root = if arg.is_empty() {
                cwd
            } else {
                let p = std::path::PathBuf::from(arg);
                if p.is_absolute() { p } else { cwd.join(p) }
            };
            if !root.is_dir() {
                send_notice(
                    &tx,
                    &crate::t!("builtins-index-invalid", path = root.display().to_string()),
                );
                return true;
            }
            match crate::index::index_root(store.conn(), &root).await {
                Ok(stats) => {
                    send_notice(
                        &tx,
                        &crate::t!(
                            "builtins-index-done",
                            scanned = stats.scanned,
                            updated = stats.updated,
                            skipped = stats.skipped,
                            removed = stats.removed,
                            chunks = stats.chunks
                        ),
                    );
                }
                Err(err) => {
                    send_notice(
                        &tx,
                        &crate::t!("store-query-error", error = err.user_message()),
                    );
                }
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

/// 将会话编号（1-based）或会话 ID 解析为实际会话 ID。
async fn resolve_session_id(store: &Arc<HistoryStore>, arg: &str) -> Result<String, String> {
    let sessions = store
        .list_sessions()
        .await
        .map_err(|err| crate::t!("store-query-error", error = err.user_message()))?;
    if let Ok(n) = arg.parse::<usize>() {
        return sessions
            .get(n.wrapping_sub(1))
            .map(|s| s.id.clone())
            .ok_or_else(|| crate::t!("builtins-session-not-found", input = arg));
    }
    // 精确匹配，否则唯一前缀匹配
    if let Some(s) = sessions.iter().find(|s| s.id == arg) {
        return Ok(s.id.clone());
    }
    let mut matches = sessions.iter().filter(|s| s.id.starts_with(arg));
    match (matches.next(), matches.next()) {
        (Some(s), None) => Ok(s.id.clone()),
        (None, _) => Err(crate::t!("builtins-session-not-found", input = arg)),
        _ => Err(crate::t!("builtins-session-ambiguous", input = arg)),
    }
}

/// 提取消息的角色标签和首行文本摘要（最多 72 字符）。
fn message_summary(msg: &Message) -> (&'static str, String) {
    use rig::message::{AssistantContent, UserContent};
    match msg {
        Message::System { content } => ("system", truncate_preview(content, 72)),
        Message::User { content } => {
            for item in content.iter() {
                match item {
                    UserContent::Text(t) => return ("user", truncate_preview(&t.text, 72)),
                    UserContent::ToolResult(_) => {
                        return ("user", crate::t!("builtins-history-tool-result"));
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
                        return ("assistant", truncate_preview(&t.text, 72));
                    }
                    AssistantContent::ToolCall(tc) => {
                        return ("assistant", format!("[tool: {}]", tc.function.name));
                    }
                    AssistantContent::Reasoning(r) => {
                        let text = r.display_text();
                        return ("assistant", truncate_preview(&text, 72));
                    }
                    _ => {}
                }
            }
            ("assistant", String::new())
        }
    }
}

/// 截取首行文本，超过 `max` 字符时截断并加省略号。
fn truncate_preview(text: &str, max: usize) -> String {
    let first_line = text.lines().next().unwrap_or(text);
    if first_line.chars().count() <= max {
        first_line.to_string()
    } else {
        let truncated: String = first_line.chars().take(max - 1).collect();
        format!("{truncated}\u{2026}")
    }
}

fn send_notice(tx: &mpsc::UnboundedSender<OutputItem>, msg: &str) {
    // 接收端已关闭时忽略，后续 send 也会失败。
    let _ = tx.send(OutputItem::Notice(msg.to_string()));
}
