use crate::agent::DynamicAgent;
use crate::cli::command::Args;
use crate::pipeline::inject::{CWD_PARAM, inject};
use crate::pipeline::paginate::paginate;
use crate::shared::constants;
use crate::shared::error::TogiError;
use crate::store::HistoryStore;
use crate::tools::modify::Modify;
use crate::tools::read::Read;
use crate::tools::shell::Shell;
use crate::tools::{ToolEffect, ToolRegistry};
use crate::ui::ErrorInfo;
use crate::ui::interaction::{OutputItem, Session};
use crate::ui::theme::CatppuccinFlavor;
use rig::message::Message;
use rig::providers::deepseek::DEEPSEEK_V4_PRO;
use rig::tool::ToolDyn;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{RwLock, mpsc, watch};
use tokio_util::sync::CancellationToken;

type History = Arc<RwLock<Arc<[Message]>>>;
type UiSender = mpsc::UnboundedSender<OutputItem>;
type SessionId = Arc<RwLock<Arc<str>>>;

#[derive(Clone)]
struct AppController {
    agent: Arc<DynamicAgent>,
    history: History,
    store: Option<Arc<HistoryStore>>,
    session_id: SessionId,
    registry: Arc<ToolRegistry>,
    cancel_tx: watch::Sender<bool>,
    task_cancel: CancellationToken,
    submitting: Arc<AtomicBool>,
}

impl AppController {
    fn new(
        agent: Arc<DynamicAgent>,
        history: History,
        store: Option<Arc<HistoryStore>>,
        session_id: SessionId,
        registry: Arc<ToolRegistry>,
        cancel_tx: watch::Sender<bool>,
        task_cancel: CancellationToken,
    ) -> Self {
        Self {
            agent,
            history,
            store,
            session_id,
            registry,
            cancel_tx,
            task_cancel,
            submitting: Arc::new(AtomicBool::new(false)),
        }
    }

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

    /// 将当前历史落盘；存储不可用或保存失败时仅提示，不影响对话。
    async fn persist_history(&self, session_id: &str, tx: &UiSender) {
        let Some(store) = &self.store else {
            return;
        };
        let hist = self.history.read().await;
        if let Err(err) = store.save(session_id, &hist).await {
            let _ = tx.send(OutputItem::Notice(crate::t!(
                "store-save-error",
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
        let hist = Arc::clone(&*self.history.read().await);
        let _ = self.cancel_tx.send_replace(false);
        let cancel_rx = self.cancel_tx.subscribe();
        let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let ui_tx = tx.clone();
        let registry = Arc::clone(&self.registry);
        let forward_task = tokio::spawn(async move {
            // 工具调用与结果在流中严格 1:1 交替出现（rig-core streaming.rs
            // 在每个 tool_call 之后立即执行并 yield 对应的 tool_result，
            // 然后才进入下一个 tool_call）。因此只需记录最近一个 tool_call
            // 的副作用类别。
            let mut pending_effect: Option<ToolEffect> = None;
            while let Some(event) = agent_rx.recv().await {
                let _ = ui_tx.send(crate::transform::to_output(
                    event,
                    &mut pending_effect,
                    &registry,
                ));
            }
        });

        let result = tokio::select! {
            _ = self.task_cancel.cancelled() => {
                forward_task.abort();
                let _ = tx.send(OutputItem::Notice(crate::t!("app-cancelled")));
                let _ = tx.send(OutputItem::Done);
                return;
            }
            r = self.agent.stream_chat(&message, hist, agent_tx, cancel_rx) => r,
        };
        let _ = forward_task.await;

        match result {
            Ok(updated_history) => {
                *self.history.write().await = Arc::from(updated_history);
                self.persist_history(&session_id, &tx).await;
                let _ = tx.send(OutputItem::Done);
            }
            Err(failure) => {
                // 失败同样保留并落盘部分历史：已完成的工具往返对后续对话有效。
                *self.history.write().await = Arc::from(failure.history);
                self.persist_history(&session_id, &tx).await;
                let _ = tx.send(OutputItem::Error(ErrorInfo::from_error(&failure.source)));
                let _ = tx.send(OutputItem::Done);
            }
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

async fn preload_highlighting() {
    // 在后台线程预加载语法数据，避免首次 Markdown 渲染时卡顿。
    // 使用 spawn_blocking 确保在当前 tokio runtime 的阻塞线程池中执行，
    // 并 await 等待完成，保证首次渲染前数据已就绪。
    let _ = tokio::task::spawn_blocking(|| {
        let _ = crate::ui::style::syntax_set();
        let _ = crate::ui::style::highlight_theme();
    })
    .await;
}

fn build_tools(cwd: &Path) -> (Vec<Box<dyn ToolDyn>>, ToolRegistry) {
    let mut registry = ToolRegistry::new();

    // 在工具被 inject / paginate 包装之前注册其副作用分类器。
    // 包装后的 trait object 无法访问静态分类函数，因此必须
    // 在此处捕获函数指针。
    registry.register::<Read>();
    registry.register::<Modify>();
    registry.register::<Shell>();

    let tools = paginate(
        constants::DEFAULT_PAGE_LINES,
        inject(
            serde_json::Map::from_iter([(CWD_PARAM.into(), cwd.display().to_string().into())]),
            vec![
                Box::new(Read) as Box<dyn ToolDyn>,
                Box::new(Modify),
                Box::new(Shell),
            ],
        ),
    );

    (tools, registry)
}

fn build_agent(
    args: &Args,
    config: &crate::config::Config,
    tools: Vec<Box<dyn ToolDyn>>,
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

/// 初始化持久化存储并恢复上次会话的历史记录。
///
/// 数据库不可用时静默降级为纯内存模式，不影响正常对话功能。
async fn init_history() -> (History, Option<Arc<HistoryStore>>, SessionId) {
    let session_id: SessionId =
        Arc::new(RwLock::new(Arc::from(crate::store::default_session_id())));
    let Some(db_path) = crate::store::default_db_path() else {
        return (
            Arc::new(RwLock::new(Arc::from(Vec::new()))),
            None,
            session_id,
        );
    };
    match HistoryStore::open(&db_path).await {
        Ok(store) => {
            let store = Arc::new(store);
            let sid = session_id.read().await.clone();
            let messages = store.load(&sid).await.unwrap_or_else(|err| {
                eprintln!(
                    "{}",
                    crate::t!("store-load-error", error = err.user_message())
                );
                Vec::new()
            });
            let history = Arc::new(RwLock::new(Arc::from(messages)));
            (history, Some(store), session_id)
        }
        Err(err) => {
            eprintln!(
                "{}",
                crate::t!(
                    "store-open-error",
                    path = db_path.display().to_string(),
                    error = err.user_message()
                )
            );
            (
                Arc::new(RwLock::new(Arc::from(Vec::new()))),
                None,
                session_id,
            )
        }
    }
}

pub async fn run() -> crate::shared::error::Result<()> {
    let args = Args::parse();
    let config = crate::config::Config::load()?;
    apply_theme(&args, &config)?;
    preload_highlighting().await;

    let cwd = std::env::current_dir().map_err(|source| crate::shared::error::AppError::Io {
        context: crate::t!("app-context-get-cwd"),
        source,
    })?;
    let (tools, registry) = build_tools(&cwd);
    let agent = Arc::new(build_agent(&args, &config, tools)?);
    let (history, store, session_id) = init_history().await;
    let mut session = Session::new()?;

    let global_cancel = CancellationToken::new();
    let controller = AppController::new(
        agent,
        history,
        store,
        session_id,
        Arc::new(registry),
        session.cancel_sender(),
        global_cancel.clone(),
    );
    let session_cancel = global_cancel.clone();

    let session_task = tokio::spawn(async move {
        let result = session
            .run(
                move |message, tx| {
                    controller.spawn_submission(message, tx);
                },
                session_cancel,
            )
            .await;
        if let Err(e) = result {
            eprintln!(
                "{}",
                crate::t!(
                    "app-session-error",
                    error = crate::shared::error::TogiError::user_message(&e)
                )
            );
        }
        if let Err(e) = session.save_history() {
            eprintln!(
                "{}",
                crate::t!(
                    "app-history-save-error",
                    error = crate::shared::error::TogiError::user_message(&e)
                )
            );
        }
    });

    // Ctrl-C 处理统一由 Session 内部的双击检测负责。
    // Session 退出后，取消所有进行中的后台任务。
    session_task.await?;
    global_cancel.cancel();

    Ok(())
}
