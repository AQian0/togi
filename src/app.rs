use crate::agent::{AgentEvent, AgentSection, DynamicAgent};
use crate::command::Args;
use crate::constants;
use crate::inject::{CWD_PARAM, Injection, inject};
use crate::paginate::paginate;
use crate::tools::modify::Modify;
use crate::tools::read::Read;
use crate::tools::shell::Shell;
use crate::ui::interaction::{OutputItem, SectionKind, Session};
use crate::ui::output::ErrorInfo;
use crate::ui::theme::CatppuccinFlavor;
use rig::message::Message;
use rig::providers::deepseek::DEEPSEEK_V4_PRO;
use rig::tool::ToolDyn;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc, watch};
use tokio_util::sync::CancellationToken;

type History = Arc<Mutex<Vec<Message>>>;
type UiSender = mpsc::UnboundedSender<OutputItem>;

#[derive(Clone)]
struct AppController {
    agent: Arc<DynamicAgent>,
    history: History,
    cancel_tx: watch::Sender<bool>,
    task_cancel: CancellationToken,
}

impl AppController {
    fn new(
        agent: Arc<DynamicAgent>,
        history: History,
        cancel_tx: watch::Sender<bool>,
        task_cancel: CancellationToken,
    ) -> Self {
        Self {
            agent,
            history,
            cancel_tx,
            task_cancel,
        }
    }

    fn spawn_submission(&self, message: String, tx: UiSender) {
        let this = self.clone();
        tokio::spawn(async move {
            this.handle_submission(message, tx).await;
        });
    }

    async fn handle_submission(self, message: String, tx: UiSender) {
        if message.starts_with('/') {
            let handled =
                crate::builtins::handle_command(&message, tx.clone(), &self.history).await;
            if handled {
                return;
            }
        }

        let hist = self.history.lock().await.clone();
        let _ = self.cancel_tx.send_replace(false);
        let cancel_rx = self.cancel_tx.subscribe();
        let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let ui_tx = tx.clone();
        let forward_task = tokio::spawn(async move {
            while let Some(event) = agent_rx.recv().await {
                let _ = ui_tx.send(output_from_agent_event(event));
            }
        });

        let result = tokio::select! {
            _ = self.task_cancel.cancelled() => {
                forward_task.abort();
                let _ = tx.send(OutputItem::Notice("已取消。".to_string()));
                let _ = tx.send(OutputItem::Done);
                return;
            }
            r = self.agent.stream_chat(&message, hist, agent_tx, cancel_rx) => r,
        };
        let _ = forward_task.await;

        match result {
            Ok(updated_history) => {
                *self.history.lock().await = updated_history;
                let _ = tx.send(OutputItem::Done);
            }
            Err(err) => {
                let _ = tx.send(OutputItem::Error(ErrorInfo::from_error(&err)));
                let _ = tx.send(OutputItem::Done);
            }
        }
    }
}

fn output_from_agent_event(event: AgentEvent) -> OutputItem {
    match event {
        AgentEvent::Section(section) => OutputItem::Section(match section {
            AgentSection::Reasoning => SectionKind::Reasoning,
            AgentSection::Answer => SectionKind::Answer,
        }),
        AgentEvent::Text(text) => OutputItem::Chunk(text),
        AgentEvent::ToolCall { name, arguments } => {
            let summary = crate::ui::summarize::summarize_call(&name, &arguments);
            OutputItem::ToolCall { name, summary }
        }
        AgentEvent::ToolResult(text) => OutputItem::ToolResult(text),
        AgentEvent::Notice(text) => OutputItem::Notice(text),
    }
}

fn apply_theme(args: &Args, config: &crate::config::Config) -> crate::error::Result<()> {
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

fn build_tools(cwd: &Path) -> Vec<Box<dyn ToolDyn>> {
    let injected = Injection::new().value(CWD_PARAM, cwd.display().to_string());
    paginate(
        constants::DEFAULT_PAGE_LINES,
        inject(
            injected,
            vec![
                Box::new(Read) as Box<dyn ToolDyn>,
                Box::new(Modify),
                Box::new(Shell),
            ],
        ),
    )
}

fn build_agent(
    args: &Args,
    config: &crate::config::Config,
    tools: Vec<Box<dyn ToolDyn>>,
) -> crate::error::Result<DynamicAgent> {
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
        .map_err(|source| crate::error::AppError::DefaultModelInit { source })
    }
}

pub async fn run() -> crate::error::Result<()> {
    let args = Args::parse();
    let config = crate::config::Config::load()?;
    apply_theme(&args, &config)?;
    preload_highlighting().await;

    let cwd = std::env::current_dir()?;
    let agent = Arc::new(build_agent(&args, &config, build_tools(&cwd))?);
    let history = Arc::new(Mutex::new(Vec::new()));
    let mut session = Session::new()?;

    let global_cancel = CancellationToken::new();
    let controller = AppController::new(
        agent,
        history,
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
            eprintln!("Session 错误：{e}");
        }
        if let Err(e) = session.save_history() {
            eprintln!("无法保存输入历史：{e}");
        }
    });

    // Ctrl-C 处理统一由 Session 内部的双击检测负责。
    // Session 退出后，取消所有进行中的后台任务。
    session_task.await?;
    global_cancel.cancel();

    Ok(())
}
