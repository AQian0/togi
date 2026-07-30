use crate::agent::{ChatOutcome, DynamicAgent, TurnFailure};
use crate::cli::command::Args;
use crate::context::{ContextCheckpoint, ContextInput, ContextPolicy};
use crate::pipeline::confirm::{ApprovalPolicy, confirm};
use crate::pipeline::inject::{CWD_PARAM, inject};
use crate::pipeline::paginate::paginate;
use crate::shared::constants;
use crate::shared::error::TogiError;
use crate::store::HistoryStore;
use crate::tools::agent::AgentTool;
use crate::tools::modify::Modify;
use crate::tools::read::Read;
use crate::tools::shell::Shell;
use crate::tools::{ToolEffect, ToolRegistry};
use crate::ui::ErrorInfo;
use crate::ui::OutputItem;
use crate::ui::session::Session;
use crate::ui::theme::CatppuccinFlavor;
use rig::message::Message;
use rig::providers::deepseek::DEEPSEEK_V4_PRO;
use rig::tool::DynamicTool;
use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{RwLock, mpsc, watch};
use tokio_util::sync::CancellationToken;

type History = Arc<RwLock<Arc<[Message]>>>;
type UiSender = mpsc::UnboundedSender<OutputItem>;
type SessionId = Arc<RwLock<String>>;

#[derive(Clone)]
struct AppController {
    agent: Arc<DynamicAgent>,
    history: History,
    store: Option<Arc<HistoryStore>>,
    session_id: SessionId,
    context: Arc<RwLock<ContextCheckpoint>>,
    context_policy: Option<ContextPolicy>,
    registry: Arc<ToolRegistry>,
    cancel_tx: watch::Sender<bool>,
    task_cancel: CancellationToken,
    submitting: Arc<AtomicBool>,
}

impl AppController {
    fn spawn_submission(&self, message: String, tx: UiSender) {
        // 防御深度：UI 层已用 Session::submitting 串行化提交，此处兜底——
        // 防止未来绕过 UI 的调用路径导致两个 handle_submission 并发竞争
        // history / session_id。
        // ponytail: 若 handle_submission panic，Done 与原子位同时卡住，
        // 会话本已不可用，故不做 panic 安全复位。
        if self.submitting.swap(true, Ordering::AcqRel) {
            let _ = tx.send(OutputItem::Notice(crate::t!("app-busy")));
            // 必须补发 Done：UI 在 Enter 时已置 submitting=true，靠 Done 复位。
            let _ = tx.send(OutputItem::Done);
            return;
        }
        let this = self.clone();
        tokio::spawn(async move {
            let submitting = Arc::clone(&this.submitting);
            this.handle_submission(message, tx).await;
            submitting.store(false, Ordering::Release);
        });
    }

    /// 将当前历史与上下文 checkpoint 落盘（先消息后 checkpoint）；
    /// 存储不可用或保存失败时仅提示，不影响对话。
    async fn persist_history(&self, session_id: &str, tx: &UiSender) {
        let Some(store) = &self.store else {
            return;
        };
        {
            let hist = self.history.read().await;
            if let Err(err) = store.save(session_id, &hist).await {
                tracing::error!(error = %err.user_message(), "history save failed");
                let _ = tx.send(OutputItem::Notice(crate::t!(
                    "store-save-error",
                    error = err.user_message()
                )));
            }
        }
        let checkpoint = self.context.read().await.clone();
        if checkpoint.is_empty() {
            return;
        }
        if let Err(err) = store.save_context(session_id, &checkpoint).await {
            tracing::error!(error = %err.user_message(), "context save failed");
            let _ = tx.send(OutputItem::Notice(crate::t!(
                "context-save-error",
                error = err.user_message()
            )));
        }
    }

    async fn handle_submission(self, message: String, tx: UiSender) {
        if message.starts_with('/') {
            // 记录命令前的会话 ID，用于检测是否发生了切换
            let old_sid = self.session_id.read().await.clone();
            let handled = crate::cli::builtins::handle_command(
                &message,
                tx.clone(),
                &self.history,
                self.store.as_ref(),
                &self.session_id,
                &self.context,
            )
            .await;
            if handled {
                // 检查会话是否已切换，如果是则通知 UI 重放历史
                let new_sid = self.session_id.read().await.clone();
                if *new_sid != *old_sid {
                    let hist = self.history.read().await.clone();
                    let _ = tx.send(OutputItem::ReplaceHistory(hist.to_vec()));
                }
                return;
            }
        }

        let session_id = self.session_id.read().await.clone();
        tracing::debug!(session = %session_id, input_len = message.len(), "submission started");
        let hist = Arc::clone(&*self.history.read().await);
        let _ = self.cancel_tx.send_replace(false);
        let cancel_rx = self.cancel_tx.subscribe();
        let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let ui_tx = tx.clone();
        let registry = Arc::clone(&self.registry);
        let forward_task = tokio::spawn(async move {
            // 工具调用与结果按 rig 生成的 internal_call_id 配对
            // （随机、跨流唯一），父子代理事件交错时仍能正确分类。
            let mut pending_effects: std::collections::HashMap<String, ToolEffect> =
                std::collections::HashMap::new();
            while let Some(event) = agent_rx.recv().await {
                let _ = ui_tx.send(crate::transform::to_output(
                    event,
                    &mut pending_effects,
                    &registry,
                ));
            }
        });

        let result = drive_chat(
            self.agent.stream_chat(
                &message,
                hist,
                ContextInput {
                    policy: self.context_policy,
                    checkpoint: self.context.read().await.clone(),
                },
                agent_tx,
                cancel_rx,
            ),
            &self.task_cancel,
            &self.cancel_tx,
            &tx,
        )
        .await;
        let _ = forward_task.await;

        let Some(result) = result else {
            // 取消收尾超时：进程正在退出，直接结束。
            let _ = tx.send(OutputItem::Done);
            return;
        };
        match result {
            Ok(outcome) => {
                tracing::debug!(messages = outcome.history.len(), "submission completed");
                *self.history.write().await = Arc::from(outcome.history);
                *self.context.write().await = outcome.context;
                self.persist_history(&session_id, &tx).await;
                let _ = tx.send(OutputItem::Done);
            }
            Err(failure) => {
                // 失败同样保留并落盘部分历史：已完成的工具往返对后续对话有效。
                tracing::error!(error = %failure.source, "submission failed");
                *self.history.write().await = Arc::from(failure.history);
                *self.context.write().await = failure.context;
                self.persist_history(&session_id, &tx).await;
                let _ = tx.send(OutputItem::Error(ErrorInfo::from_error(&failure.source)));
                let _ = tx.send(OutputItem::Done);
            }
        }
    }
}

/// 等待一次提交完成；全局取消（退出）时复用 Esc 取消路径收尾：
/// 向 `cancel_tx` 发取消信号，让 `stream_chat` 自行整理本轮部分历史
/// 并正常返回，而不是直接 drop future——否则本轮已完成的工具往返
/// （副作用已写盘）会从历史中凭空消失且不落盘。
///
/// 超过 [`constants::CANCEL_DRAIN_TIMEOUT`] 未收尾则放弃，返回 `None`。
async fn drive_chat(
    chat: impl Future<Output = Result<ChatOutcome, TurnFailure>>,
    task_cancel: &CancellationToken,
    cancel_tx: &watch::Sender<bool>,
    tx: &UiSender,
) -> Option<Result<ChatOutcome, TurnFailure>> {
    tokio::pin!(chat);
    tokio::select! {
        biased;
        r = &mut chat => Some(r),
        _ = task_cancel.cancelled() => {
            let _ = tx.send(OutputItem::Notice(crate::t!("app-cancelled")));
            let _ = cancel_tx.send_replace(true);
            tokio::time::timeout(constants::CANCEL_DRAIN_TIMEOUT, &mut chat)
                .await
                .ok()
        }
    }
}

fn apply_theme(args: &Args, config: &crate::config::Config) -> crate::shared::error::Result<()> {
    let theme_name = args
        .theme
        .as_deref()
        .or(config.system.theme.as_deref())
        .unwrap_or("Latte");
    let flavor: CatppuccinFlavor = theme_name.parse()?;
    crate::ui::theme::set_flavor(flavor);
    Ok(())
}

/// 启动时预加载语法数据，避免首次 Markdown 渲染时卡顿。
fn preload_highlighting() {
    let _ = crate::ui::style::syntax_set();
    let _ = crate::ui::style::highlight_theme();
}

fn build_tools(
    cwd: &Path,
    subagent_model: &str,
    api_key: Option<&str>,
    max_multi_turn: u32,
    approval_policy: ApprovalPolicy,
) -> (Vec<DynamicTool>, ToolRegistry) {
    let mut registry = ToolRegistry::new();

    // 在工具被 inject / paginate 包装之前注册其副作用分类器。
    // 包装后的 trait object 无法访问静态分类函数，因此必须
    // 在此处捕获函数指针。
    registry.register::<Read>();
    registry.register::<Modify>();
    registry.register::<Shell>();
    registry.register::<AgentTool>();

    let tools = vec![
        crate::pipeline::adapt(Read),
        crate::pipeline::adapt(Modify),
        crate::pipeline::adapt(Shell),
        crate::pipeline::adapt(
            AgentTool::new(
                cwd,
                subagent_model.to_string(),
                api_key.map(str::to_string),
                max_multi_turn,
            )
            .with_approval_policy(approval_policy.clone()),
        ),
    ];
    let tools = inject(
        serde_json::Map::from_iter([(CWD_PARAM.into(), cwd.display().to_string().into())]),
        tools,
    );
    let tools = paginate(constants::DEFAULT_PAGE_LINES, tools);
    let tools = confirm(tools, registry.clone(), approval_policy);

    (tools, registry)
}

fn build_agent(
    args: &Args,
    config: &crate::config::Config,
    tools: Vec<DynamicTool>,
) -> crate::shared::error::Result<DynamicAgent> {
    let model_name = args.model.as_ref().or(config.system.model.as_ref());
    let api_key = args.api_key.as_deref();
    let preamble = config.effective_preamble();
    if let Some(model_name) = model_name {
        DynamicAgent::build(
            model_name,
            preamble,
            tools,
            api_key,
            config.effective_max_multi_turn(),
        )
        .map_err(Into::into)
    } else {
        DynamicAgent::build(
            DEEPSEEK_V4_PRO,
            preamble,
            tools,
            api_key,
            config.effective_max_multi_turn(),
        )
        .map_err(|source| crate::shared::error::AppError::DefaultModelInit { source })
    }
}

/// 初始化持久化存储并恢复上次会话的历史记录与上下文 checkpoint。
///
/// 数据库不可用时静默降级为纯内存模式，不影响正常对话功能。
async fn init_history() -> (
    History,
    Option<Arc<HistoryStore>>,
    SessionId,
    Arc<RwLock<ContextCheckpoint>>,
) {
    let session_id: SessionId = Arc::new(RwLock::new(constants::DEFAULT_SESSION_ID.to_string()));
    let empty_history = || Arc::new(RwLock::new(Arc::from(Vec::new())));
    let empty_context = || Arc::new(RwLock::new(ContextCheckpoint::default()));
    let Some(db_path) = crate::store::default_db_path() else {
        return (empty_history(), None, session_id, empty_context());
    };
    match HistoryStore::open(&db_path).await {
        Ok(store) => {
            let store = Arc::new(store);
            let sid = session_id.read().await.clone();
            let report = store.load(&sid).await.unwrap_or_else(|err| {
                eprintln!(
                    "{}",
                    crate::t!("store-load-error", error = err.user_message())
                );
                crate::store::LoadReport::default()
            });
            tracing::debug!(session = %sid, messages = report.messages.len(), dropped = report.dropped_rows, "history loaded");
            if report.dropped_rows > 0 {
                eprintln!(
                    "{}",
                    crate::t!("store-history-truncated", dropped = report.dropped_rows)
                );
            }
            let checkpoint = store.load_context(&sid).await.unwrap_or_else(|err| {
                eprintln!(
                    "{}",
                    crate::t!("context-load-error", error = err.user_message())
                );
                ContextCheckpoint::default()
            });
            // 加载截断可能使 checkpoint 越过历史末尾，作废重建。
            let checkpoint = if checkpoint.is_valid_for(report.messages.len()) {
                checkpoint
            } else {
                ContextCheckpoint::default()
            };
            let history = Arc::new(RwLock::new(Arc::from(report.messages)));
            (
                history,
                Some(store),
                session_id,
                Arc::new(RwLock::new(checkpoint)),
            )
        }
        Err(err) => {
            tracing::error!(path = %db_path.display(), error = %err.user_message(), "store open failed");
            eprintln!(
                "{}",
                crate::t!(
                    "store-open-error",
                    path = db_path.display().to_string(),
                    error = err.user_message()
                )
            );
            (empty_history(), None, session_id, empty_context())
        }
    }
}

pub async fn run() -> crate::shared::error::Result<()> {
    let args = <Args as clap::Parser>::parse();
    let config = crate::config::Config::load()?;
    let context_policy = config.effective_context_policy()?;
    apply_theme(&args, &config)?;
    preload_highlighting();

    let cwd = std::env::current_dir().map_err(|source| crate::shared::error::AppError::Io {
        context: crate::t!("app-context-get-cwd"),
        source,
    })?;
    tracing::debug!(cwd = %cwd.display(), model = ?args.model, "app starting");
    let approval_policy = ApprovalPolicy::new(config.approval.always_allow.iter().cloned());
    let (tools, registry) = build_tools(
        &cwd,
        // 子代理缺省模型跟随主代理（profile 可覆盖）。
        args.model
            .as_ref()
            .or(config.system.model.as_ref())
            .map_or(DEEPSEEK_V4_PRO, String::as_str),
        args.api_key.as_deref(),
        config.effective_max_multi_turn(),
        approval_policy.clone(),
    );
    let agent = Arc::new(build_agent(&args, &config, tools)?);
    let (history, store, session_id, context) = init_history().await;
    let mut session = Session::new(approval_policy)?;

    let global_cancel = CancellationToken::new();
    let controller = AppController {
        agent,
        history,
        store,
        session_id,
        context,
        context_policy,
        registry: Arc::new(registry),
        cancel_tx: session.cancel_sender(),
        task_cancel: global_cancel.clone(),
        submitting: Arc::new(AtomicBool::new(false)),
    };
    let result = session
        .run(
            move |message, tx| {
                controller.spawn_submission(message, tx);
            },
            global_cancel.clone(),
        )
        .await;
    if let Err(e) = result {
        tracing::error!(error = %e.user_message(), "session error");
        eprintln!(
            "{}",
            crate::t!("app-session-error", error = e.user_message())
        );
    }
    if let Err(e) = session.save_history() {
        tracing::error!(error = %e.user_message(), "history save failed");
        eprintln!(
            "{}",
            crate::t!("app-history-save-error", error = e.user_message())
        );
    }

    // Ctrl-C 处理统一由 Session 内部的双击检测负责。
    // Session 退出后，取消所有进行中的后台任务。
    global_cancel.cancel();

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 全局取消时复用 Esc 取消路径：等待 chat 收尾并返回部分历史，
    /// 而不是直接 drop future 丢弃本轮已完成的工具往返。
    #[tokio::test]
    async fn drive_chat_drains_partial_history_on_cancel() {
        let (cancel_tx, mut cancel_rx) = watch::channel(false);
        let token = CancellationToken::new();
        let (tx, _rx) = mpsc::unbounded_channel();
        let chat = async move {
            // 模拟 stream_chat 的取消收尾：收到取消信号后返回部分历史
            cancel_rx.changed().await.unwrap();
            Ok(ChatOutcome {
                history: vec![Message::user("partial")],
                context: ContextCheckpoint::default(),
            })
        };
        token.cancel();
        let result = drive_chat(chat, &token, &cancel_tx, &tx).await;
        let outcome = result.expect("cancel should drain, not drop");
        let outcome = outcome.expect("chat should succeed");
        assert_eq!(outcome.history.len(), 1);
    }
}
