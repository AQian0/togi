use crate::context::{
    ContextCheckpoint, ContextInput, ContextPolicy, ContextRuntime, Decision, UsageSample,
};
use crate::shared::constants;
use crate::shared::error::{ErrorKind, TogiError};
use futures::StreamExt;
use itertools::Itertools;
use rig::OneOrMany;
use rig::agent::{
    AgentHook, Flow, HookContext, MultiTurnStreamItem, RequestPatch, StepEvent, StepEventKind,
};
use rig::client::{CompletionClient, ProviderClient};
use rig::completion::CompletionModel;
use rig::completion::message::ToolResultContent;
use rig::message::{AssistantContent, Message, Reasoning, Text, ToolCall, ToolResult, UserContent};
use rig::streaming::{StreamedAssistantContent, StreamedUserContent, StreamingPrompt};
use rig::tool::ToolDyn;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, watch};

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error(
        "could not identify the provider for model \"{model}\". Supported providers: {supported}"
    )]
    UnknownProvider { model: String, supported: String },

    #[error("{env} environment variable not set: {source}")]
    MissingApiKey {
        env: &'static str,
        #[source]
        source: rig::client::ProviderClientError,
    },

    #[error("could not initialize {provider} provider: {source}")]
    ProviderInit {
        provider: &'static str,
        #[source]
        source: rig::client::ProviderClientError,
    },

    #[error("model streaming failed: {source}")]
    Stream {
        #[source]
        source: rig::agent::StreamingError,
    },

    #[error("model stream stalled: no data received for {secs}s")]
    Stalled { secs: u64 },
}

impl TogiError for AgentError {
    fn code(&self) -> &'static str {
        match self {
            Self::UnknownProvider { .. } => "agent.unknown_provider",
            Self::MissingApiKey { .. } => "agent.missing_api_key",
            Self::ProviderInit { .. } => "agent.provider_init",
            Self::Stream { .. } => "agent.stream",
            Self::Stalled { .. } => "agent.stalled",
        }
    }

    fn kind(&self) -> ErrorKind {
        match self {
            Self::UnknownProvider { .. } => ErrorKind::InvalidArgument,
            Self::MissingApiKey { .. } | Self::ProviderInit { .. } | Self::Stream { .. } => {
                ErrorKind::External
            }
            Self::Stalled { .. } => ErrorKind::Timeout,
        }
    }

    fn user_message(&self) -> String {
        match self {
            Self::UnknownProvider { model, supported } => {
                crate::t!(
                    "agent-unknown-provider",
                    model = model.clone(),
                    supported = supported.clone()
                )
            }
            Self::MissingApiKey { env, source: _ } => {
                crate::t!("agent-missing-api-key", env = *env)
            }
            Self::ProviderInit { provider, source } => {
                crate::t!(
                    "agent-provider-init",
                    provider = *provider,
                    source = source.to_string()
                )
            }
            Self::Stream { source } => {
                crate::t!("agent-stream-error", source = source.to_string())
            }
            Self::Stalled { secs } => {
                crate::t!("agent-stalled", secs = *secs)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentSection {
    Reasoning,
    Answer,
}

/// 一轮对话失败：错误本身 + 失败前已产生的部分历史。
///
/// `history` 保留已完成的工具往返与已生成的文本（零进展时等于原历史），
/// 避免工具副作用已写入磁盘、而模型上下文里这一轮凭空消失的状态不一致。
#[derive(Debug)]
pub struct TurnFailure {
    pub source: AgentError,
    pub history: Vec<Message>,
    /// 失败时的上下文 checkpoint：取消、provider 错误或部分工具轮次后
    /// 仍能保存有效的压缩状态。
    pub context: ContextCheckpoint,
    /// 本轮失败前是否见过工具调用——见过则副作用可能已发生，不可安全重试。
    saw_tool_call: bool,
}

/// 一次提交的结果：完整历史 + 上下文 checkpoint。
#[derive(Debug)]
pub struct ChatOutcome {
    pub history: Vec<Message>,
    pub context: ContextCheckpoint,
}

#[derive(Debug)]
pub enum AgentEvent {
    Section(AgentSection),
    Text(String),
    ToolCall {
        name: String,
        arguments: serde_json::Value,
        /// Rig-generated unique identifier for this tool call, correlating
        /// the call with its result and deltas.
        internal_call_id: String,
    },
    ToolResult {
        text: String,
        /// Rig-generated unique identifier matching the originating
        /// `ToolCall::internal_call_id`.
        internal_call_id: String,
    },
    ApprovalRequest {
        name: String,
        arguments: serde_json::Value,
        response: tokio::sync::oneshot::Sender<crate::tools::ApprovalDecision>,
    },
    Notice(String),
    /// 子代理产生的事件：`depth` 为展示深度（1 = 主代理的直接子代理）。
    /// 嵌套子代理的事件会被多层 `Child` 包裹，转换层取最内层深度。
    Child { depth: u32, event: Box<AgentEvent> },
}

pub type AgentEventSender = tokio::sync::mpsc::UnboundedSender<AgentEvent>;
type ChatFuture = Pin<Box<dyn Future<Output = Result<ChatOutcome, TurnFailure>> + Send>>;
type ChatFn = dyn Fn(
        String,
        Arc<[Message]>,
        ContextInput,
        AgentEventSender,
        watch::Receiver<bool>,
        u32,
    ) -> ChatFuture
    + Send
    + Sync;

/// 流式过程中累积本轮新产生的消息，供取消 / 出错时收尾成合法历史。
#[derive(Default)]
struct PartialTurn {
    /// 推理增量缓冲：`ReasoningDelta` 只给字符串，提交时合并为一条 [`Reasoning`]。
    reasoning: String,
    /// 当前未提交的 assistant 内容。
    assistant: Vec<AssistantContent>,
    /// 本批次已到达的工具结果。与 rig 的规范历史对齐：同一批调用的所有
    /// result 合并为一条 user 消息（provider 对并行调用的要求）。
    results: Vec<UserContent>,
    /// 已提交的新消息：assistant（含工具调用）与 tool result 用户消息严格成对交替。
    committed: Vec<Message>,
    /// 是否见过工具调用（含后来被丢弃的悬空调用）。
    saw_tool_call: bool,
}

impl PartialTurn {
    fn push_reasoning_delta(&mut self, delta: &str) {
        self.flush_results();
        self.reasoning.push_str(delta);
    }

    fn push_reasoning(&mut self, reasoning: Reasoning) {
        self.flush_results();
        self.flush_reasoning();
        self.assistant.push(AssistantContent::Reasoning(reasoning));
    }

    fn push_text(&mut self, text: String) {
        self.flush_results();
        self.flush_reasoning();
        self.assistant.push(AssistantContent::Text(Text::new(text)));
    }

    fn push_tool_call(&mut self, tool_call: ToolCall) {
        self.saw_tool_call = true;
        self.flush_results();
        self.flush_reasoning();
        self.assistant.push(AssistantContent::ToolCall(tool_call));
    }

    fn push_tool_result(&mut self, tool_result: ToolResult) {
        self.saw_tool_call = true;
        self.commit_assistant();
        self.results.push(UserContent::ToolResult(tool_result));
    }

    fn flush_reasoning(&mut self) {
        if !self.reasoning.is_empty() {
            self.assistant
                .push(AssistantContent::Reasoning(Reasoning::new(&self.reasoning)));
            self.reasoning.clear();
        }
    }

    fn flush_results(&mut self) {
        if self.results.is_empty() {
            return;
        }
        let content =
            OneOrMany::many(std::mem::take(&mut self.results)).expect("results non-empty");
        self.committed.push(Message::User { content });
    }

    fn commit_assistant(&mut self) {
        self.flush_reasoning();
        if self.assistant.is_empty() {
            return;
        }
        let content = OneOrMany::many(std::mem::take(&mut self.assistant))
            .expect("assistant content non-empty");
        self.committed
            .push(Message::Assistant { id: None, content });
    }

    /// 收尾为完整历史：原历史 + 用户输入 + 本轮新消息；零进展时原样返回。
    ///
    /// 未拿到结果的悬空工具调用会被丢弃——多数 provider 拒绝存在
    /// tool call 而无对应 result 的历史。
    fn finish(mut self, input: &str, history: &[Message]) -> Vec<Message> {
        while matches!(self.assistant.last(), Some(AssistantContent::ToolCall(_))) {
            self.assistant.pop();
        }
        self.commit_assistant();
        self.flush_results();
        if self.committed.is_empty() {
            return history.to_vec();
        }
        let mut out = history.to_vec();
        out.push(Message::user(input));
        out.extend(self.committed);
        out
    }
}

/// 提取 rig 错误自带的规范历史（如 `MaxTurnsError` / `PromptCancelled`），
/// 存在时优先于本地重建。
fn canonical_history(err: &rig::agent::StreamingError) -> Option<Vec<Message>> {
    let rig::agent::StreamingError::Prompt(e) = err else {
        return None;
    };
    match &**e {
        rig::completion::PromptError::MaxTurnsError { chat_history, .. }
        | rig::completion::PromptError::UnknownToolCall { chat_history, .. } => {
            Some((**chat_history).clone())
        }
        rig::completion::PromptError::PromptCancelled { chat_history, .. } => {
            Some(chat_history.clone())
        }
        _ => None,
    }
}

/// 瞬态错误的类别——决定重试退避的基准时长。
///
/// 分类基于 rig 的错误 API（`provider_response_status`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransientKind {
    /// HTTP 429 限流：需要更长的冷却时间。
    ///
    /// ponytail: rig 0.40 的错误 API 只保留状态码与 body、不暴露响应头，
    /// 拿不到 Retry-After，退化为更长的指数退避；rig 暴露头信息后再接入。
    RateLimited,
    /// HTTP 5xx：服务端临时故障。
    Server,
    /// 无状态码的传输层失败（连接错误、超时、流停滞）。
    Transport,
}

impl TransientKind {
    /// 判断错误是否瞬态：HTTP 429 / 5xx，或无状态码的传输层失败。
    fn classify(err: &AgentError) -> Option<Self> {
        match err {
            AgentError::Stalled { .. } => Some(Self::Transport),
            AgentError::Stream { source } => {
                let rig::agent::StreamingError::Completion(e) = source else {
                    return None;
                };
                match e.provider_response_status() {
                    Some(status) if status == http::StatusCode::TOO_MANY_REQUESTS => {
                        Some(Self::RateLimited)
                    }
                    Some(status) if status.is_server_error() => Some(Self::Server),
                    Some(_) => None,
                    None => matches!(e, rig::completion::CompletionError::HttpError(_))
                        .then_some(Self::Transport),
                }
            }
            _ => None,
        }
    }
}

/// 普通瞬态错误（5xx / 传输失败）的首次退避时长。
const BACKOFF_BASE: Duration = Duration::from_secs(1);

/// HTTP 429 的首次退避时长（限流需要更长冷却）。
const RATE_LIMIT_BACKOFF_BASE: Duration = Duration::from_secs(5);

/// 指数退避增长因子。
const BACKOFF_FACTOR: u32 = 2;

/// 单次退避时长上限。
const BACKOFF_MAX: Duration = Duration::from_secs(30);

/// 计算第 `attempt` 次失败后的退避时长：指数退避加抖动，封顶 [`BACKOFF_MAX`]。
fn backoff_delay(kind: TransientKind, attempt: u32) -> Duration {
    let base = match kind {
        TransientKind::RateLimited => RATE_LIMIT_BACKOFF_BASE,
        TransientKind::Server | TransientKind::Transport => BACKOFF_BASE,
    };
    let shift = attempt.saturating_sub(1);
    jittered(base * BACKOFF_FACTOR.pow(shift)).min(BACKOFF_MAX)
}

/// 在 [0.5x, 1.5x) 区间内抖动，避免固定节拍。
fn jittered(delay: Duration) -> Duration {
    let millis = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX);
    // ponytail: 用系统时间纳秒做廉价随机源，避免引入 rand 依赖；
    // 单进程重试无需加密级随机性。
    let entropy = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::from(d.subsec_nanos()))
        .unwrap_or(0);
    Duration::from_millis(millis / 2 + entropy % (millis - millis / 2 + 1))
}

/// 决定一次失败是否重试：仅在未见过工具调用（无副作用风险）、
/// 错误瞬态、且尝试次数未用尽时重试。
///
/// ponytail: 见过工具调用后不重试原输入——那需要 rig 暴露请求边界以续传；
/// 目前靠部分历史落盘 + 用户继续对话来兜底。
fn retry_delay(err: &AgentError, saw_tool_call: bool, attempt: u32) -> Option<Duration> {
    if attempt >= MAX_ATTEMPTS || saw_tool_call {
        return None;
    }
    TransientKind::classify(err).map(|kind| backoff_delay(kind, attempt))
}

fn cancel_return(
    tx: &AgentEventSender,
    input: &str,
    history: &[Message],
    partial: PartialTurn,
) -> Vec<Message> {
    let _ = tx.send(AgentEvent::Notice(crate::t!("agent-esc-interrupted")));
    partial.finish(input, history)
}

/// 单次提交内激活的上下文管理：策略 + 跨重试共享的运行期状态。
struct ActiveContext {
    policy: ContextPolicy,
    runtime: Arc<Mutex<ContextRuntime>>,
}

/// 提取运行期 checkpoint；上下文管理未启用时透传输入 checkpoint。
async fn current_checkpoint(
    active: &Option<ActiveContext>,
    fallback: &ContextCheckpoint,
) -> ContextCheckpoint {
    match active {
        Some(active) => active.runtime.lock().await.checkpoint.clone(),
        None => fallback.clone(),
    }
}

/// 上下文溢出错误的常见 provider 文本特征。
const OVERFLOW_PATTERNS: &[&str] = &[
    "context_length_exceeded",
    "maximum context length",
    "context window",
    "prompt too long",
    "too many tokens",
];

/// 判断错误是否为 provider 的上下文窗口溢出（依据错误文本，含响应 body）。
fn is_context_overflow(err: &AgentError) -> bool {
    let AgentError::Stream { source } = err else {
        return false;
    };
    let mut text = source.to_string();
    let body = match source {
        rig::agent::StreamingError::Completion(e) => e.provider_response_body(),
        rig::agent::StreamingError::Prompt(e) => e.provider_response_body(),
        _ => None,
    };
    if let Some(body) = body {
        text.push_str(body);
    }
    let text = text.to_lowercase();
    OVERFLOW_PATTERNS.iter().any(|p| text.contains(p))
}

fn ensure_section(current: &mut AgentSection, target: AgentSection, tx: &AgentEventSender) {
    if *current != target {
        *current = target;
        let _ = tx.send(AgentEvent::Section(target));
    }
}
/// 最大尝试次数（含首次请求）。
const MAX_ATTEMPTS: u32 = 3;

#[tracing::instrument(skip_all, fields(input_len = input.len(), history_len = history.len()))]
pub async fn stream_chat<M: CompletionModel + 'static>(
    agent: &rig::agent::Agent<M>,
    input: &str,
    history: &Arc<[Message]>,
    context: ContextInput,
    tx: AgentEventSender,
    mut cancel_rx: watch::Receiver<bool>,
    max_multi_turn: u32,
) -> Result<ChatOutcome, TurnFailure> {
    let mut attempt = 1;
    let mut overflow_retried = false;
    let active = context.policy.map(|policy| ActiveContext {
        policy,
        runtime: Arc::new(Mutex::new(ContextRuntime {
            checkpoint: context.checkpoint.clone(),
            ..Default::default()
        })),
    });
    loop {
        match stream_once(
            agent,
            input,
            history,
            &active,
            &tx,
            &mut cancel_rx,
            max_multi_turn,
        )
        .await
        {
            Ok(updated) => {
                return Ok(ChatOutcome {
                    history: updated,
                    context: current_checkpoint(&active, &context.checkpoint).await,
                });
            }
            Err(failure) => {
                // 上下文溢出恢复：尚未执行 ToolCall 时强制压缩并最多重试一次。
                // 已执行 ToolCall 则不自动重试（副作用可能已发生），保守失败。
                if !failure.saw_tool_call
                    && !overflow_retried
                    && let Some(active_ctx) = &active
                    && is_context_overflow(&failure.source)
                {
                    overflow_retried = true;
                    tracing::debug!("context overflow, forcing compaction retry");
                    let mut rt = active_ctx.runtime.lock().await;
                    rt.force_compaction = true;
                    rt.overflow_error = Some(failure.source.to_string());
                    let _ = tx.send(AgentEvent::Notice(crate::t!("context-overflow-retry")));
                    continue;
                }
                let mut failure = failure;
                failure.context = current_checkpoint(&active, &context.checkpoint).await;
                let Some(delay) = retry_delay(&failure.source, failure.saw_tool_call, attempt)
                else {
                    tracing::debug!(attempt, error = %failure.source, "giving up: not retryable");
                    return Err(failure);
                };
                tracing::debug!(attempt, delay_ms = delay.as_millis() as u64, error = %failure.source, "transient error, retrying");
                let _ = tx.send(AgentEvent::Notice(crate::t!(
                    "agent-retrying",
                    attempt = attempt + 1,
                    max = MAX_ATTEMPTS,
                    delay = delay.as_secs()
                )));
                attempt += 1;
                tokio::select! {
                    biased;
                    _ = cancel_rx.changed() => {
                        return Ok(ChatOutcome {
                            history: cancel_return(&tx, input, history, PartialTurn::default()),
                            context: current_checkpoint(&active, &context.checkpoint).await,
                        });
                    }
                    _ = tokio::time::sleep(delay) => {}
                }
            }
        }
    }
}

/// 构造流停滞失败：保留部分历史，错误标记为停滞（瞬态）。
fn stall_failure(
    partial: PartialTurn,
    input: &str,
    history: &[Message],
    waited: Duration,
) -> TurnFailure {
    tracing::debug!(waited_ms = waited.as_millis() as u64, "stream stalled");
    let saw_tool_call = partial.saw_tool_call;
    TurnFailure {
        source: AgentError::Stalled {
            secs: waited.as_secs(),
        },
        history: partial.finish(input, history),
        // 占位，由 stream_chat 统一填充运行期 checkpoint。
        context: ContextCheckpoint::default(),
        saw_tool_call,
    }
}

async fn stream_once<M: CompletionModel + 'static>(
    agent: &rig::agent::Agent<M>,
    input: &str,
    history: &[Message],
    active_ctx: &Option<ActiveContext>,
    tx: &AgentEventSender,
    cancel_rx: &mut watch::Receiver<bool>,
    max_multi_turn: u32,
) -> Result<Vec<Message>, TurnFailure> {
    let mut section = AgentSection::Answer;
    let mut partial = PartialTurn::default();
    let mut final_history: Option<Vec<Message>> = None;
    // 每次调用注入运行时扩展：事件通道与取消信号，供 agent 等
    // 需要运行时上下文的工具经 `call_with_extensions` 取用。
    let mut extensions = rig::tool::ToolCallExtensions::new();
    extensions.insert(tx.clone());
    extensions.insert(cancel_rx.clone());
    let stream_request = agent
        .stream_prompt(input)
        .history(history.to_vec())
        .max_turns(max_multi_turn as usize)
        .tool_concurrency(constants::TOOL_CONCURRENCY)
        .tool_extensions(extensions);
    // 上下文管理启用时挂载 hook：每次 completion 前检查预算并按需压缩。
    let stream_request = match active_ctx {
        Some(active) => stream_request.add_hook(ContextHook {
            model: Arc::clone(&agent.model),
            policy: active.policy,
            runtime: Arc::clone(&active.runtime),
            tx: tx.clone(),
        }),
        None => stream_request,
    };
    let mut stream = tokio::select! {
        biased;
        _ = cancel_rx.changed() => return Ok(cancel_return(tx, input, history, partial)),
        stream = tokio::time::timeout(constants::STREAM_STALL_TIMEOUT, stream_request) => match stream {
            Ok(stream) => stream,
            Err(_elapsed) => {
                return Err(stall_failure(partial, input, history, constants::STREAM_STALL_TIMEOUT));
            }
        },
    };
    // 已收到调用、未收到结果的工具数：等待工具结果期间放宽停滞超时，
    // 因为工具（如 shell）可能合法运行至自身超时上限。
    let mut pending_tool_calls: u32 = 0;
    loop {
        let stall = if pending_tool_calls > 0 {
            constants::TOOL_RESULT_GRACE
        } else {
            constants::STREAM_STALL_TIMEOUT
        };
        let item = tokio::select! {
            biased;
            _ = cancel_rx.changed() => return Ok(cancel_return(tx, input, history, partial)),
            item = tokio::time::timeout(stall, stream.next()) => item,
        };
        let item = match item {
            Ok(item) => item,
            Err(_elapsed) => return Err(stall_failure(partial, input, history, stall)),
        };
        match item {
            Some(Ok(MultiTurnStreamItem::StreamAssistantItem(content))) => match content {
                StreamedAssistantContent::Reasoning(reasoning) => {
                    ensure_section(&mut section, AgentSection::Reasoning, tx);
                    let _ = tx.send(AgentEvent::Text(reasoning.display_text().clone()));
                    partial.push_reasoning(reasoning);
                }
                StreamedAssistantContent::ReasoningDelta { reasoning, .. } => {
                    ensure_section(&mut section, AgentSection::Reasoning, tx);
                    partial.push_reasoning_delta(&reasoning);
                    let _ = tx.send(AgentEvent::Text(reasoning));
                }
                StreamedAssistantContent::Text(text) => {
                    ensure_section(&mut section, AgentSection::Answer, tx);
                    partial.push_text(text.text.clone());
                    let _ = tx.send(AgentEvent::Text(text.text));
                }
                StreamedAssistantContent::ToolCall {
                    tool_call,
                    internal_call_id,
                } => {
                    tracing::debug!(
                        name = %tool_call.function.name,
                        arguments = %tool_call.function.arguments,
                        call_id = %internal_call_id,
                        "tool call"
                    );
                    pending_tool_calls = pending_tool_calls.saturating_add(1);
                    partial.push_tool_call(tool_call.clone());
                    let _ = tx.send(AgentEvent::ToolCall {
                        name: tool_call.function.name,
                        arguments: tool_call.function.arguments,
                        internal_call_id,
                    });
                }
                StreamedAssistantContent::Unknown(value) => {
                    let _ = tx.send(AgentEvent::Notice(crate::t!(
                        "agent-unhandled-output",
                        value = value.to_string()
                    )));
                }
                _ => {}
            },
            Some(Ok(MultiTurnStreamItem::StreamUserItem(StreamedUserContent::ToolResult {
                tool_result,
                internal_call_id,
            }))) => {
                pending_tool_calls = pending_tool_calls.saturating_sub(1);
                partial.push_tool_result(tool_result.clone());
                let text: String = tool_result
                    .content
                    .iter()
                    .filter_map(|c| match c {
                        ToolResultContent::Text(t) => Some(t.text.as_str()),
                        _ => None,
                    })
                    .join("\n");
                tracing::debug!(call_id = %internal_call_id, result_len = text.len(), "tool result");
                let _ = tx.send(AgentEvent::ToolResult {
                    text,
                    internal_call_id,
                });
            }
            Some(Ok(MultiTurnStreamItem::FinalResponse(final_response))) => {
                if let Some(new_messages) = final_response.messages() {
                    // rig 0.40 的 messages 只含本轮新增（prompt + 各轮消息），
                    // 不含输入历史——必须拼回完整 transcript。
                    let mut full = history.to_vec();
                    full.extend_from_slice(new_messages);
                    final_history = Some(full);
                }
            }
            // 每个 completion 的实际 usage：供同一提交内的后续工具轮次
            // 用最新样本做预算投影（全零视为 provider 未上报，保留估算兜底）。
            Some(Ok(MultiTurnStreamItem::CompletionCall(call))) => {
                if let Some(active) = active_ctx
                    && let Some(input_tokens) = crate::context::effective_input_tokens(&call.usage)
                {
                    tracing::trace!(input_tokens, "completion usage");
                    let mut rt = active.runtime.lock().await;
                    rt.usage = Some(UsageSample {
                        input_tokens,
                        estimated_request_tokens: rt.last_request_estimate,
                    });
                }
            }
            Some(Ok(_)) => {}
            Some(Err(err)) => {
                tracing::error!(error = %err, "stream error");
                let saw_tool_call = partial.saw_tool_call;
                let history = match canonical_history(&err) {
                    Some(h) => h,
                    None => partial.finish(input, history),
                };
                return Err(TurnFailure {
                    history,
                    saw_tool_call,
                    // 占位，由 stream_chat 统一填充运行期 checkpoint。
                    context: ContextCheckpoint::default(),
                    source: AgentError::Stream { source: err },
                });
            }
            None => break,
        }
    }
    Ok(final_history.unwrap_or_else(|| partial.finish(input, history)))
}

/// 每次 completion 前检查上下文预算并按需滚动压缩的 request hook。
///
/// 只挂到主请求；摘要请求直接用 `CompletionModel::completion_request`，
/// 不经过 agent 循环，避免递归压缩。
struct ContextHook<M: CompletionModel> {
    model: Arc<M>,
    policy: ContextPolicy,
    runtime: Arc<Mutex<ContextRuntime>>,
    tx: AgentEventSender,
}

/// 调用当前模型生成滚动摘要：独立 preamble、空 history、无工具、
/// 最大输出 `min(8192, reserve_tokens / 2)`。
async fn summarize<M: CompletionModel>(
    model: &M,
    policy: &ContextPolicy,
    input: String,
) -> Result<String, crate::context::SummaryError> {
    let response = model
        .completion_request(Message::user(input))
        .preamble(crate::context::SUMMARY_PREAMBLE.to_string())
        .max_tokens(policy.summary_max_tokens())
        .send()
        .await?;
    let text = response
        .choice
        .iter()
        .filter_map(|c| match c {
            AssistantContent::Text(t) => Some(t.text.as_str()),
            _ => None,
        })
        .join("\n");
    let text = text.trim().to_string();
    if text.is_empty() {
        Err(crate::context::SummaryError::Empty)
    } else {
        Ok(text)
    }
}

impl<M: CompletionModel> AgentHook<M> for ContextHook<M> {
    async fn on_event(&self, _ctx: &HookContext, event: StepEvent<'_, M>) -> Flow {
        let StepEvent::CompletionCall {
            prompt, history, ..
        } = event
        else {
            return Flow::cont();
        };
        let mut rt = self.runtime.lock().await;
        let checkpoint = rt.checkpoint.clone();
        let active = crate::context::build_active_history(&checkpoint, history);
        let estimate =
            crate::context::estimate_history(&active) + crate::context::estimate_message(prompt);
        rt.last_request_estimate = estimate;
        match crate::context::decide(
            &self.policy,
            &checkpoint,
            rt.usage,
            rt.force_compaction,
            history,
            prompt,
            estimate,
        ) {
            Decision::None => Flow::cont(),
            Decision::PatchActive => Flow::patch_request(RequestPatch::new().history(active)),
            Decision::Terminate => {
                // 强制压缩仍无合法切点：返回原始 provider 错误及上下文说明。
                // （force_compaction 只与 overflow_error 一起设置，None 分支不可达。）
                let error = rt.overflow_error.take().unwrap_or_default();
                Flow::terminate(crate::t!("context-compact-impossible", error = error))
            }
            Decision::Compact(k) => {
                // force 标志一次性消费：无论本次摘要成败。
                rt.force_compaction = false;
                rt.overflow_error = None;
                let covered = checkpoint.covered_messages;
                let input = crate::context::build_summary_prompt(
                    checkpoint.summary.as_deref(),
                    &history[covered..k],
                );
                match summarize(self.model.as_ref(), &self.policy, input).await {
                    Ok(summary) => {
                        tracing::debug!(covered = k, previous = covered, "context compacted");
                        // 摘要成功后原子更新 summary 与 covered_messages。
                        rt.checkpoint = ContextCheckpoint {
                            summary: Some(summary),
                            covered_messages: k,
                        };
                        let active = crate::context::build_active_history(&rt.checkpoint, history);
                        rt.last_request_estimate = crate::context::estimate_history(&active)
                            + crate::context::estimate_message(prompt);
                        let _ = self.tx.send(AgentEvent::Notice(crate::t!(
                            "context-compacted",
                            count = k - covered
                        )));
                        Flow::patch_request(RequestPatch::new().history(active))
                    }
                    Err(err) => {
                        tracing::error!(error = %err, "context compaction failed");
                        // 摘要失败：checkpoint 不推进，按原活动历史发送；
                        // 若 provider 随后溢出，由 overflow 恢复路径处理。
                        let _ = self.tx.send(AgentEvent::Notice(crate::t!(
                            "context-compact-failed",
                            error = err.to_string()
                        )));
                        Flow::patch_request(RequestPatch::new().history(active))
                    }
                }
            }
        }
    }

    fn observes(&self, kind: StepEventKind) -> bool {
        !matches!(
            kind,
            StepEventKind::TextDelta | StepEventKind::ToolCallDelta
        )
    }
}

pub struct DynamicAgent {
    chat: Box<ChatFn>,
    max_multi_turn: u32,
}

impl DynamicAgent {
    pub async fn stream_chat(
        &self,
        input: &str,
        history: Arc<[Message]>,
        context: ContextInput,
        tx: AgentEventSender,
        cancel_rx: watch::Receiver<bool>,
    ) -> Result<ChatOutcome, TurnFailure> {
        (self.chat)(
            input.to_string(),
            history,
            context,
            tx,
            cancel_rx,
            self.max_multi_turn,
        )
        .await
    }
}

macro_rules! providers {
    (
        $(
            $variant:ident,
            client = $client:ty,
            env    = $env:expr,
            prefixes = [$($prefix:expr),* $(,)?]
        ),* $(,)?
    ) => {
        #[derive(Debug, Clone, Copy)]
        enum Provider { $($variant,)* }
        fn resolve_provider(model: &str) -> Option<(&'static str, Provider)> {
            $(
                if [$($prefix),*].iter().any(|p| model.starts_with(p)) {
                    return Some(($env, Provider::$variant));
                }
            )*
            None
        }
        fn provider_list() -> String {
            let mut parts = Vec::new();
            $(
                parts.push(format!("{} → {}",
                    [$($prefix),*].join("/"),
                    stringify!($variant),
                ));
            )*
            parts.join(", ")
        }
        impl DynamicAgent {
            pub fn build(
                model: &str,
                preamble: &str,
                tools: Vec<Box<dyn ToolDyn>>,
                api_key: Option<&str>,
                max_multi_turn: u32,
            ) -> Result<Self, AgentError> {
                let (_, provider) = resolve_provider(model).ok_or_else(|| {
                    AgentError::UnknownProvider {
                        model: model.to_string(),
                        supported: provider_list(),
                    }
                })?;
                match provider {
                    $(
                        Provider::$variant => {
                            let client = if let Some(key) = api_key {
                                <$client>::new(key).map_err(|source| AgentError::ProviderInit {
                                    provider: stringify!($variant),
                                    source: source.into(),
                                })?
                            } else {
                                <$client>::from_env().map_err(|source| AgentError::MissingApiKey {
                                    env: $env,
                                    source,
                                })?
                            };
                            tracing::debug!(model, provider = stringify!($variant), "agent built");
                            let inner = Arc::new(
                                client.agent(model)
                                    .preamble(preamble)
                                    .tools(tools)
                                    .build(),
                            );
                            return Ok(DynamicAgent {
                                chat: Box::new(
                                    move |input,
                                          history: Arc<[Message]>,
                                          context: ContextInput,
                                          tx,
                                          cancel_rx,
                                          max_multi_turn| {
                                        let agent = Arc::clone(&inner);
                                        Box::pin(async move {
                                            crate::agent::stream_chat(
                                                &agent,
                                                &input,
                                                &history,
                                                context,
                                                tx,
                                                cancel_rx,
                                                max_multi_turn,
                                            )
                                            .await
                                        })
                                    },
                                ),
                                max_multi_turn,
                            });
                        }
                    )*
                }
            }
        }
    };
}
providers! {
    DeepSeek,
        client = rig::providers::deepseek::Client,
        env    = "DEEPSEEK_API_KEY",
        prefixes = ["deepseek-"],
    OpenAI,
        client = rig::providers::openai::CompletionsClient,
        env    = "OPENAI_API_KEY",
        prefixes = ["gpt-", "o1", "o3", "o4"],
    Anthropic,
        client = rig::providers::anthropic::Client,
        env    = "ANTHROPIC_API_KEY",
        prefixes = ["claude-"],
    Gemini,
        client = rig::providers::gemini::Client,
        env    = "GEMINI_API_KEY",
        prefixes = ["gemini-"],
    Xai,
        client = rig::providers::xai::Client,
        env    = "XAI_API_KEY",
        prefixes = ["grok-"],
    Mistral,
        client = rig::providers::mistral::Client,
        env    = "MISTRAL_API_KEY",
        prefixes = ["mistral-", "ministral-", "codestral-", "pixtral-"],
    Cohere,
        client = rig::providers::cohere::Client,
        env    = "COHERE_API_KEY",
        prefixes = ["command-"],
    Perplexity,
        client = rig::providers::perplexity::Client,
        env    = "PERPLEXITY_API_KEY",
        prefixes = ["sonar"],
}

#[cfg(test)]
mod tests {
    use super::*;
    use rig::message::{ToolFunction, ToolResultContent};

    fn tool_call(id: &str, name: &str) -> ToolCall {
        ToolCall::new(
            id.into(),
            ToolFunction::new(name.into(), serde_json::json!({})),
        )
    }

    fn tool_result(id: &str, text: &str) -> ToolResult {
        ToolResult {
            id: id.into(),
            call_id: None,
            content: OneOrMany::one(ToolResultContent::Text(Text::new(text))),
        }
    }

    fn stream_err(status: http::StatusCode) -> AgentError {
        AgentError::Stream {
            source: rig::agent::StreamingError::Completion(
                rig::completion::CompletionError::from_http_response(status, "boom"),
            ),
        }
    }

    #[test]
    fn finish_without_progress_returns_history_unchanged() {
        let history = vec![Message::user("hi")];
        let out = PartialTurn::default().finish("hello", &history);
        assert_eq!(out, history);
    }

    #[test]
    fn finish_with_text_appends_input_and_assistant() {
        let mut p = PartialTurn::default();
        p.push_text("正在处理".into());
        let out = p.finish("做点事", &[]);
        assert_eq!(out.len(), 2);
        assert!(matches!(&out[0], Message::User { .. }));
        let Message::Assistant { content, .. } = &out[1] else {
            panic!("expected assistant message");
        };
        assert!(
            content
                .iter()
                .any(|c| matches!(c, AssistantContent::Text(t) if t.text == "正在处理"))
        );
    }

    #[test]
    fn finish_preserves_completed_tool_pair() {
        let mut p = PartialTurn::default();
        p.push_tool_call(tool_call("call-1", "read"));
        p.push_tool_result(tool_result("call-1", "文件内容"));
        p.push_text("读完了".into());
        let out = p.finish("读文件", &[]);
        // input + assistant(tool call) + user(result) + assistant(text)
        assert_eq!(out.len(), 4);
        let Message::Assistant { content, .. } = &out[1] else {
            panic!("expected assistant message");
        };
        assert!(
            content
                .iter()
                .any(|c| matches!(c, AssistantContent::ToolCall(_)))
        );
        let Message::User { content } = &out[2] else {
            panic!("expected user message");
        };
        assert!(
            content
                .iter()
                .any(|c| matches!(c, UserContent::ToolResult(r) if r.id == "call-1"))
        );
    }

    #[test]
    fn finish_strips_dangling_tool_call() {
        let mut p = PartialTurn::default();
        p.push_text("我查一下".into());
        p.push_tool_call(tool_call("call-1", "shell"));
        // 取消 / 出错发生在工具结果返回前
        let out = p.finish("跑个命令", &[]);
        assert_eq!(out.len(), 2);
        let Message::Assistant { content, .. } = &out[1] else {
            panic!("expected assistant message");
        };
        assert!(
            !content
                .iter()
                .any(|c| matches!(c, AssistantContent::ToolCall(_)))
        );
        assert!(
            content
                .iter()
                .any(|c| matches!(c, AssistantContent::Text(t) if t.text == "我查一下"))
        );
    }

    #[test]
    fn finish_drops_turn_containing_only_dangling_call() {
        let mut p = PartialTurn::default();
        p.push_tool_call(tool_call("call-1", "shell"));
        let out = p.finish("跑个命令", &[]);
        assert!(out.is_empty());
    }

    #[test]
    fn finish_batches_parallel_results_into_single_user_message() {
        let mut p = PartialTurn::default();
        p.push_tool_call(tool_call("call-1", "read"));
        p.push_tool_call(tool_call("call-2", "read"));
        p.push_tool_result(tool_result("call-1", "内容一"));
        p.push_tool_result(tool_result("call-2", "内容二"));
        let out = p.finish("读两个文件", &[]);
        // input + assistant(两个 call) + 一条 user(两个 result 合并)
        assert_eq!(out.len(), 3);
        let Message::Assistant { content, .. } = &out[1] else {
            panic!("expected assistant message");
        };
        assert_eq!(
            content
                .iter()
                .filter(|c| matches!(c, AssistantContent::ToolCall(_)))
                .count(),
            2
        );
        let Message::User { content } = &out[2] else {
            panic!("expected user message");
        };
        assert_eq!(
            content
                .iter()
                .filter(|c| matches!(c, UserContent::ToolResult(_)))
                .count(),
            2
        );
    }

    #[test]
    fn dangling_call_marks_tool_call_seen_despite_unchanged_history() {
        let mut p = PartialTurn::default();
        assert!(!p.saw_tool_call);
        p.push_tool_call(tool_call("call-1", "shell"));
        // 悬空调用被丢弃、历史不变，但已见过工具调用——不可安全重试
        assert!(p.saw_tool_call);
    }

    #[test]
    fn canonical_history_extracted_from_max_turns_error() {
        let err = rig::agent::StreamingError::Prompt(Box::new(
            rig::completion::PromptError::MaxTurnsError {
                max_turns: 10,
                chat_history: vec![Message::user("q"), Message::assistant("a")].into(),
                prompt: Message::user("q").into(),
            },
        ));
        let history = canonical_history(&err).expect("should extract canonical history");
        assert_eq!(history, vec![Message::user("q"), Message::assistant("a")]);
    }

    #[test]
    fn canonical_history_returns_none_for_completion_error() {
        let err = rig::agent::StreamingError::Completion(
            rig::completion::CompletionError::from_http_response(
                http::StatusCode::TOO_MANY_REQUESTS,
                "boom",
            ),
        );
        assert!(canonical_history(&err).is_none());
    }

    #[test]
    fn reasoning_deltas_merge_into_single_reasoning_before_text() {
        let mut p = PartialTurn::default();
        p.push_reasoning_delta("先想");
        p.push_reasoning_delta("一下");
        p.push_text("答案".into());
        let out = p.finish("问", &[]);
        let Message::Assistant { content, .. } = &out[1] else {
            panic!("expected assistant message");
        };
        let items: Vec<&AssistantContent> = content.iter().collect();
        assert_eq!(items.len(), 2);
        assert!(matches!(items[0], AssistantContent::Reasoning(_)));
        assert!(matches!(items[1], AssistantContent::Text(_)));
    }

    #[test]
    fn transient_statuses_are_classified() {
        assert_eq!(
            TransientKind::classify(&stream_err(http::StatusCode::TOO_MANY_REQUESTS)),
            Some(TransientKind::RateLimited)
        );
        assert_eq!(
            TransientKind::classify(&stream_err(http::StatusCode::SERVICE_UNAVAILABLE)),
            Some(TransientKind::Server)
        );
        assert_eq!(
            TransientKind::classify(&stream_err(http::StatusCode::INTERNAL_SERVER_ERROR)),
            Some(TransientKind::Server)
        );
        assert_eq!(
            TransientKind::classify(&AgentError::Stalled { secs: 120 }),
            Some(TransientKind::Transport)
        );
    }

    #[test]
    fn permanent_errors_are_not_transient() {
        assert_eq!(
            TransientKind::classify(&stream_err(http::StatusCode::BAD_REQUEST)),
            None
        );
        assert_eq!(
            TransientKind::classify(&stream_err(http::StatusCode::UNAUTHORIZED)),
            None
        );
        assert_eq!(
            TransientKind::classify(&AgentError::UnknownProvider {
                model: "x".into(),
                supported: "y".into(),
            }),
            None
        );
    }

    #[test]
    fn retry_aborts_after_tool_call_seen() {
        let err = stream_err(http::StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(retry_delay(&err, true, 1), None);
    }

    #[test]
    fn retry_allowed_before_any_tool_call() {
        let err = stream_err(http::StatusCode::SERVICE_UNAVAILABLE);
        assert!(retry_delay(&err, false, 1).is_some());
    }

    #[test]
    fn retry_aborts_when_attempts_exhausted() {
        let err = stream_err(http::StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(retry_delay(&err, false, MAX_ATTEMPTS), None);
    }

    #[test]
    fn rate_limit_backoff_is_longer_than_server_backoff() {
        let rate_limited = backoff_delay(TransientKind::RateLimited, 1);
        let server = backoff_delay(TransientKind::Server, 1);
        assert!(rate_limited > server);
        assert!(server >= BACKOFF_BASE / 2);
        assert!(rate_limited >= RATE_LIMIT_BACKOFF_BASE / 2);
    }

    #[test]
    fn resolve_deepseek() {
        let (env, _) = resolve_provider("deepseek-chat").unwrap();
        assert_eq!(env, "DEEPSEEK_API_KEY");
    }

    #[test]
    fn resolve_openai_gpt() {
        let (env, _) = resolve_provider("gpt-4o").unwrap();
        assert_eq!(env, "OPENAI_API_KEY");
    }

    #[test]
    fn resolve_openai_o_series() {
        let (env, _) = resolve_provider("o1-preview").unwrap();
        assert_eq!(env, "OPENAI_API_KEY");
        let (env, _) = resolve_provider("o3-mini").unwrap();
        assert_eq!(env, "OPENAI_API_KEY");
    }

    #[test]
    fn resolve_anthropic() {
        let (env, _) = resolve_provider("claude-3-5-sonnet").unwrap();
        assert_eq!(env, "ANTHROPIC_API_KEY");
    }

    #[test]
    fn resolve_gemini() {
        let (env, _) = resolve_provider("gemini-2.0-flash").unwrap();
        assert_eq!(env, "GEMINI_API_KEY");
    }

    #[test]
    fn resolve_xai() {
        let (env, _) = resolve_provider("grok-2").unwrap();
        assert_eq!(env, "XAI_API_KEY");
    }

    #[test]
    fn resolve_mistral_variants() {
        let (env, _) = resolve_provider("mistral-large").unwrap();
        assert_eq!(env, "MISTRAL_API_KEY");
        let (env, _) = resolve_provider("codestral-latest").unwrap();
        assert_eq!(env, "MISTRAL_API_KEY");
        let (env, _) = resolve_provider("pixtral-12b").unwrap();
        assert_eq!(env, "MISTRAL_API_KEY");
    }

    #[test]
    fn resolve_cohere() {
        let (env, _) = resolve_provider("command-r-plus").unwrap();
        assert_eq!(env, "COHERE_API_KEY");
    }

    #[test]
    fn resolve_perplexity() {
        let (env, _) = resolve_provider("sonar-pro").unwrap();
        assert_eq!(env, "PERPLEXITY_API_KEY");
    }

    #[test]
    fn resolve_unknown_returns_none() {
        assert!(resolve_provider("llama-3").is_none());
        assert!(resolve_provider("unknown-model").is_none());
        assert!(resolve_provider("").is_none());
    }

    #[test]
    fn provider_list_is_not_empty() {
        let list = provider_list();
        assert!(!list.is_empty());
        assert!(list.contains("DeepSeek"));
        assert!(list.contains("OpenAI"));
        assert!(list.contains("Anthropic"));
    }

    #[test]
    fn build_unknown_model_returns_error() {
        use crate::shared::error::TogiError;
        let result = DynamicAgent::build("unknown-model", "", vec![], None, 3);
        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("expected error for unknown model"),
        };
        assert_eq!(err.code(), "agent.unknown_provider");
        assert_eq!(err.kind(), crate::shared::error::ErrorKind::InvalidArgument);
        let err_msg = err.to_string();
        assert!(
            err_msg.contains("unknown-model"),
            "unexpected error: {err_msg}"
        );
    }

    // ── 上下文窗口管理 ──────────────────────────────────────────────

    use crate::context::ContextPolicy;
    use rig::completion::{CompletionRequest, CompletionResponse, Usage};
    use rig::streaming::{RawStreamingChoice, StreamingCompletionResponse};
    use rig::test_utils::{MockAddTool, MockResponse};
    use std::collections::VecDeque;
    use std::sync::Mutex as StdMutex;

    fn overflow_err(text: &str) -> AgentError {
        AgentError::Stream {
            source: rig::agent::StreamingError::Completion(
                rig::completion::CompletionError::ProviderError(text.into()),
            ),
        }
    }

    #[test]
    fn overflow_classification_matches_common_patterns() {
        for text in [
            "error code: context_length_exceeded",
            "This model's maximum context length is 8192 tokens",
            "the context window is full",
            "prompt too long: 200000 tokens",
            "request has too many tokens",
        ] {
            assert!(is_context_overflow(&overflow_err(text)), "{text}");
        }
        assert!(!is_context_overflow(&overflow_err("rate limit exceeded")));
        assert!(!is_context_overflow(&AgentError::Stalled { secs: 1 }));
        assert!(!is_context_overflow(&stream_err(
            http::StatusCode::BAD_REQUEST
        )));
    }

    /// 脚本化模型：流式主循环与非流式摘要请求各自排队，并记录全部请求。
    /// （rig 的 MockCompletionModel 只支持单一队列，无法同时脚本化两种调用。）
    #[derive(Clone, Default)]
    struct ScriptedModel {
        state: Arc<ScriptedState>,
    }

    #[derive(Default)]
    struct ScriptedState {
        completions: StdMutex<VecDeque<Result<String, String>>>,
        streams: StdMutex<VecDeque<Vec<ScriptedStreamItem>>>,
        requests: StdMutex<Vec<CompletionRequest>>,
    }

    enum ScriptedStreamItem {
        Text(String),
        ToolCall {
            id: String,
            name: String,
            args: serde_json::Value,
        },
        Final(Usage),
        Error(String),
    }

    impl ScriptedModel {
        fn with_streams(self, streams: Vec<Vec<ScriptedStreamItem>>) -> Self {
            *self.state.streams.lock().unwrap() = streams.into();
            self
        }

        fn with_completions(self, completions: Vec<Result<String, String>>) -> Self {
            *self.state.completions.lock().unwrap() = completions.into();
            self
        }

        fn requests(&self) -> Vec<CompletionRequest> {
            self.state.requests.lock().unwrap().clone()
        }
    }

    impl CompletionModel for ScriptedModel {
        type Response = MockResponse;
        type StreamingResponse = MockResponse;
        type Client = ();

        fn make(_: &Self::Client, _: impl Into<String>) -> Self {
            Self::default()
        }

        async fn completion(
            &self,
            request: CompletionRequest,
        ) -> Result<CompletionResponse<MockResponse>, rig::completion::CompletionError> {
            use rig::completion::CompletionError;
            self.state.requests.lock().unwrap().push(request);
            let next = self
                .state
                .completions
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| CompletionError::ProviderError("no scripted completion".into()))?;
            let text = next.map_err(CompletionError::ProviderError)?;
            Ok(CompletionResponse {
                choice: OneOrMany::one(AssistantContent::text(text)),
                usage: Usage::new(),
                raw_response: MockResponse::new(),
                message_id: None,
            })
        }

        async fn stream(
            &self,
            request: CompletionRequest,
        ) -> Result<StreamingCompletionResponse<MockResponse>, rig::completion::CompletionError>
        {
            use rig::completion::CompletionError;
            self.state.requests.lock().unwrap().push(request);
            let items = self
                .state
                .streams
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| CompletionError::ProviderError("no scripted stream".into()))?;
            let choices: Vec<Result<RawStreamingChoice<MockResponse>, CompletionError>> = items
                .into_iter()
                .map(|item| match item {
                    ScriptedStreamItem::Text(t) => Ok(RawStreamingChoice::Message(t)),
                    ScriptedStreamItem::ToolCall { id, name, args } => {
                        Ok(RawStreamingChoice::ToolCall(
                            rig::streaming::RawStreamingToolCall::new(id, name, args),
                        ))
                    }
                    ScriptedStreamItem::Final(u) => Ok(RawStreamingChoice::FinalResponse(
                        MockResponse::with_usage(u),
                    )),
                    ScriptedStreamItem::Error(e) => Err(CompletionError::ProviderError(e)),
                })
                .collect();
            Ok(StreamingCompletionResponse::stream(Box::pin(
                futures::stream::iter(choices),
            )))
        }
    }

    fn usage(input: u64, output: u64) -> Usage {
        Usage {
            input_tokens: input,
            output_tokens: output,
            total_tokens: input + output,
            cached_input_tokens: 0,
            cache_creation_input_tokens: 0,
            tool_use_prompt_tokens: 0,
            reasoning_tokens: 0,
        }
    }

    fn long_history() -> Vec<Message> {
        vec![
            Message::user("u".repeat(100)),
            Message::assistant("a".repeat(100)),
            Message::user("u".repeat(100)),
            Message::assistant("a".repeat(100)),
        ]
    }

    fn test_policy() -> ContextPolicy {
        ContextPolicy {
            window_tokens: 10_000,
            reserve_tokens: 256,
            keep_recent_tokens: 60,
        }
    }

    fn ctx_input(policy: ContextPolicy) -> ContextInput {
        ContextInput {
            policy: Some(policy),
            checkpoint: ContextCheckpoint::default(),
        }
    }

    /// 测试通道：返回的 cancel_tx 必须持有到 stream_chat 结束，
    /// 否则 sender 掉线会让 changed() 立即返回（误判为取消）。
    fn channels() -> (AgentEventSender, watch::Sender<bool>, watch::Receiver<bool>) {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (cancel_tx, cancel_rx) = watch::channel(false);
        (tx, cancel_tx, cancel_rx)
    }

    /// 请求中是否包含指定文本的消息。
    fn request_contains(req: &CompletionRequest, text: &str) -> bool {
        req.chat_history.iter().any(|m| match m {
            Message::System { content } => content.contains(text),
            Message::User { content } => content.iter().any(|c| match c {
                UserContent::Text(t) => t.text.contains(text),
                UserContent::ToolResult(r) => r.content.iter().any(|rc| match rc {
                    ToolResultContent::Text(t) => t.text.contains(text),
                    _ => false,
                }),
                _ => false,
            }),
            Message::Assistant { content, .. } => content.iter().any(|c| match c {
                AssistantContent::Text(t) => t.text.contains(text),
                _ => false,
            }),
        })
    }

    /// turn 1 的 usage 必须流入 turn 2 的预算投影：同一提交内的连续
    /// CompletionCall 依次更新样本，投影超限 → turn 2 前滚动压缩，
    /// provider 收到“摘要 + 最近原文”，完整 transcript 不变。
    #[tokio::test]
    async fn usage_from_completion_call_drives_later_turn_compaction() {
        let model = ScriptedModel::default()
            .with_streams(vec![
                vec![
                    ScriptedStreamItem::ToolCall {
                        id: "tc1".into(),
                        name: "add".into(),
                        args: serde_json::json!({"x": 1, "y": 2}),
                    },
                    // turn 1 上报巨大输入 → turn 2 投影超限
                    ScriptedStreamItem::Final(usage(9_950, 10)),
                ],
                vec![
                    ScriptedStreamItem::Text("done".into()),
                    ScriptedStreamItem::Final(usage(100, 10)),
                ],
            ])
            .with_completions(vec![Ok("## Goal\n测试摘要".into())]);
        let agent = rig::agent::AgentBuilder::new(model.clone())
            .preamble("test")
            .tool(MockAddTool)
            .build();
        let (tx, _cancel_tx, cancel_rx) = channels();
        let history: Arc<[Message]> = Arc::from(long_history());
        let outcome = stream_chat(
            &agent,
            "go",
            &history,
            ctx_input(test_policy()),
            tx,
            cancel_rx,
            5,
        )
        .await
        .expect("turn should succeed");
        // checkpoint 推进，摘要内容来自脚本
        assert!(outcome.context.covered_messages > 0);
        assert_eq!(
            outcome.context.summary.as_deref(),
            Some("## Goal\n测试摘要")
        );
        // 完整 transcript：4 条输入 + go + call + result + done
        assert_eq!(outcome.history.len(), 4 + 4);
        let reqs = model.requests();
        assert_eq!(reqs.len(), 3, "stream1 + summary + stream2");
        // 摘要请求包含新淘汰历史
        assert!(request_contains(&reqs[1], "<new-history>"));
        // turn 2 请求：摘要 system 消息在内，已覆盖的原文不在
        assert!(request_contains(&reqs[2], "测试摘要"));
        assert!(!request_contains(&reqs[2], &"u".repeat(100)));
        // turn 1 请求仍是原始完整历史
        assert!(request_contains(&reqs[0], &"u".repeat(100)));
    }

    /// 初始 completion overflow → 强制压缩并重试一次 → 成功。
    #[tokio::test]
    async fn overflow_forces_compaction_and_retries_once() {
        let model = ScriptedModel::default()
            .with_streams(vec![
                vec![ScriptedStreamItem::Error(
                    "maximum context length exceeded".into(),
                )],
                vec![
                    ScriptedStreamItem::Text("ok".into()),
                    ScriptedStreamItem::Final(usage(50, 10)),
                ],
            ])
            .with_completions(vec![Ok("压缩后的摘要".into())]);
        let agent = rig::agent::AgentBuilder::new(model.clone())
            .preamble("test")
            .build();
        let (tx, _cancel_tx, cancel_rx) = channels();
        let history: Arc<[Message]> = Arc::from(long_history());
        let outcome = stream_chat(
            &agent,
            "go",
            &history,
            ctx_input(test_policy()),
            tx,
            cancel_rx,
            5,
        )
        .await
        .expect("overflow retry should succeed");
        assert!(outcome.context.covered_messages > 0);
        assert_eq!(outcome.context.summary.as_deref(), Some("压缩后的摘要"));
        let reqs = model.requests();
        assert_eq!(reqs.len(), 3, "failed stream + summary + retried stream");
        // 重试请求带摘要、不含已覆盖原文
        assert!(request_contains(&reqs[2], "压缩后的摘要"));
    }

    /// 已执行 ToolCall 后 overflow：不自动重试，保守失败。
    #[tokio::test]
    async fn no_overflow_retry_after_tool_call() {
        let model = ScriptedModel::default().with_streams(vec![
            vec![
                ScriptedStreamItem::ToolCall {
                    id: "tc1".into(),
                    name: "add".into(),
                    args: serde_json::json!({"x": 1, "y": 2}),
                },
                ScriptedStreamItem::Final(usage(100, 10)),
            ],
            vec![ScriptedStreamItem::Error(
                "context window exceeded: too many tokens".into(),
            )],
        ]);
        let agent = rig::agent::AgentBuilder::new(model.clone())
            .preamble("test")
            .tool(MockAddTool)
            .build();
        let (tx, _cancel_tx, cancel_rx) = channels();
        let history: Arc<[Message]> = Arc::from(long_history());
        let result = stream_chat(
            &agent,
            "go",
            &history,
            ctx_input(test_policy()),
            tx,
            cancel_rx,
            5,
        )
        .await;
        let failure = match result {
            Err(f) => f,
            Ok(_) => panic!("expected failure"),
        };
        assert!(is_context_overflow(&failure.source));
        // 无摘要请求、无重试
        assert_eq!(model.requests().len(), 2);
        // 部分历史保留：输入 4 + go + call + result
        assert_eq!(failure.history.len(), 4 + 3);
    }

    /// 重试后第二次仍 overflow：停止，不循环压缩。
    #[tokio::test]
    async fn second_overflow_stops_without_looping() {
        let model = ScriptedModel::default()
            .with_streams(vec![
                vec![ScriptedStreamItem::Error("prompt too long".into())],
                vec![ScriptedStreamItem::Error("prompt too long".into())],
            ])
            .with_completions(vec![Ok("部分摘要".into())]);
        let agent = rig::agent::AgentBuilder::new(model.clone())
            .preamble("test")
            .build();
        let (tx, _cancel_tx, cancel_rx) = channels();
        let history: Arc<[Message]> = Arc::from(long_history());
        let result = stream_chat(
            &agent,
            "go",
            &history,
            ctx_input(test_policy()),
            tx,
            cancel_rx,
            5,
        )
        .await;
        let failure = match result {
            Err(f) => f,
            Ok(_) => panic!("expected failure"),
        };
        assert!(is_context_overflow(&failure.source));
        // 压缩重试只有一次：failed + summary + failed，无第二个摘要请求
        assert_eq!(model.requests().len(), 3);
        // 失败仍携带压缩后的 checkpoint
        assert!(failure.context.covered_messages > 0);
        assert_eq!(failure.context.summary.as_deref(), Some("部分摘要"));
    }

    /// 强制压缩时仍无合法切点：终止并带回原始 provider 错误说明。
    #[tokio::test]
    async fn forced_compaction_without_legal_cut_returns_original_error() {
        let model = ScriptedModel::default().with_streams(vec![
            vec![ScriptedStreamItem::Error("context_length_exceeded".into())],
            vec![ScriptedStreamItem::Error("context_length_exceeded".into())],
        ]);
        let agent = rig::agent::AgentBuilder::new(model.clone())
            .preamble("test")
            .build();
        // 窗口小到连 prompt 本身都装不下 → 任何切点都非法
        let policy = ContextPolicy {
            window_tokens: 200,
            reserve_tokens: 64,
            keep_recent_tokens: 60,
        };
        let (tx, _cancel_tx, cancel_rx) = channels();
        let history: Arc<[Message]> = Arc::from(long_history());
        let result = stream_chat(
            &agent,
            &"x".repeat(500),
            &history,
            ctx_input(policy),
            tx,
            cancel_rx,
            5,
        )
        .await;
        let failure = match result {
            Err(f) => f,
            Ok(_) => panic!("expected failure"),
        };
        // 终止原因带原始 provider 错误；hook 在重试发请求前终止，无摘要请求
        assert!(
            failure
                .source
                .to_string()
                .contains("context_length_exceeded")
        );
        assert_eq!(model.requests().len(), 1);
        // 完整历史不丢（含本轮 prompt，canonical_history 语义）
        assert_eq!(failure.history.len(), 5);
    }

    /// 摘要失败：checkpoint 不推进，按原活动历史发送（本轮后续 overflow
    /// 由恢复路径处理）。
    #[tokio::test]
    async fn summary_failure_does_not_advance_checkpoint() {
        let model = ScriptedModel::default()
            .with_streams(vec![vec![
                ScriptedStreamItem::Text("done".into()),
                ScriptedStreamItem::Final(usage(100, 10)),
            ]])
            // 摘要请求直接失败
            .with_completions(vec![Err("summary boom".into())]);
        let agent = rig::agent::AgentBuilder::new(model.clone())
            .preamble("test")
            .build();
        // 极小窗口，保证触发压缩
        let policy = ContextPolicy {
            window_tokens: 200,
            reserve_tokens: 64,
            keep_recent_tokens: 60,
        };
        let (tx, _cancel_tx, cancel_rx) = channels();
        let history: Arc<[Message]> = Arc::from(long_history());
        let outcome = stream_chat(&agent, "go", &history, ctx_input(policy), tx, cancel_rx, 5)
            .await
            .expect("summary failure should not fail the turn");
        assert!(outcome.context.is_empty(), "checkpoint must not advance");
        let reqs = model.requests();
        assert_eq!(reqs.len(), 2, "summary + main stream");
        // 主请求按原活动历史发送（含全部原文）
        assert!(request_contains(&reqs[1], &"u".repeat(100)));
    }
}
