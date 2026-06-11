use crate::ui::interaction::OutputItem;
use rig::message::Message;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

pub async fn handle_command(
    line: &str,
    tx: mpsc::UnboundedSender<OutputItem>,
    history: &Arc<Mutex<Vec<Message>>>,
) -> bool {
    match line {
        "/help" => {
            send_notice(&tx, "");
            send_notice(&tx, "可用命令");
            let rows = [
                ("/help", "显示这份帮助"),
                ("/clear", "清空对话历史并重置屏幕"),
                ("/cwd", "显示当前工作目录"),
                (
                    "/exit、/quit",
                    "退出（也可用 exit / quit / 退出 或 Ctrl-C）",
                ),
            ];
            for (cmd, desc) in rows {
                send_notice(&tx, &format!("  {cmd:<14}{desc}"));
            }
            send_notice(&tx, "");
            send_notice(&tx, "快捷键");
            send_notice(&tx, "  Esc           取消当前回答 / 清空当前输入");
            send_notice(&tx, "  Ctrl-C        退出");
            send_notice(&tx, "  PageUp/PageDown  滚动对话历史");
            send_notice(&tx, "");
        }
        "/clear" => {
            let mut hist = history.lock().await;
            let count = hist.len();
            hist.clear();
            send_notice(&tx, &format!("已清空对话历史（共 {count} 条消息）。"));
        }
        "/cwd" => {
            let cwd = std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "未知".to_string());
            send_notice(&tx, &format!("当前工作目录：{cwd}"));
        }
        other => {
            send_notice(
                &tx,
                &format!("未知命令 `{other}`。输入 /help 查看可用命令。"),
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
