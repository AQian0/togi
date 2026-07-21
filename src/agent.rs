use crate::shared::constants;
use crate::shared::error::{ErrorKind, TogiError};
use futures::StreamExt;
use itertools::Itertools;
use rig::OneOrMany;
use rig::agent::MultiTurnStreamItem;
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
use tokio::sync::watch;

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
    /// 本轮失败前是否见过工具调用——见过则副作用可能已发生，不可安全重试。
    saw_tool_call: bool,
}

#[derive(Debug, Clone)]
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
    Notice(String),
}

type AgentEventSender = tokio::sync::mpsc::UnboundedSender<AgentEvent>;
type ChatFuture = Pin<Box<dyn Future<Output = Result<Vec<Message>, TurnFailure>> + Send>>;
type ChatFn = dyn Fn(String, Arc<[Message]>, AgentEventSender, watch::Receiver<bool>, u32) -> ChatFuture
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

/// 一次失败的重试决策。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RetryDecision {
    /// 放弃：永久性错误、已发生工具副作用、或已用完尝试次数。
    Abort,
    /// 延迟后重试。
    Retry { delay: Duration },
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
fn retry_decision(err: &AgentError, saw_tool_call: bool, attempt: u32) -> RetryDecision {
    if attempt >= MAX_ATTEMPTS || saw_tool_call {
        return RetryDecision::Abort;
    }
    match TransientKind::classify(err) {
        Some(kind) => RetryDecision::Retry {
            delay: backoff_delay(kind, attempt),
        },
        None => RetryDecision::Abort,
    }
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

fn ensure_section(current: &mut AgentSection, target: AgentSection, tx: &AgentEventSender) {
    if *current != target {
        *current = target;
        let _ = tx.send(AgentEvent::Section(target));
    }
}
/// 最大尝试次数（含首次请求）。
const MAX_ATTEMPTS: u32 = 3;

pub async fn stream_chat<M: CompletionModel + 'static>(
    agent: &rig::agent::Agent<M>,
    input: &str,
    history: &Arc<[Message]>,
    tx: AgentEventSender,
    mut cancel_rx: watch::Receiver<bool>,
    max_multi_turn: u32,
) -> Result<Vec<Message>, TurnFailure> {
    let mut attempt = 1;
    loop {
        match stream_once(agent, input, history, &tx, &mut cancel_rx, max_multi_turn).await {
            Ok(updated) => return Ok(updated),
            Err(failure) => {
                let RetryDecision::Retry { delay } =
                    retry_decision(&failure.source, failure.saw_tool_call, attempt)
                else {
                    return Err(failure);
                };
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
                        return Ok(cancel_return(&tx, input, history, PartialTurn::default()));
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
    let saw_tool_call = partial.saw_tool_call;
    TurnFailure {
        source: AgentError::Stalled {
            secs: waited.as_secs(),
        },
        history: partial.finish(input, history),
        saw_tool_call,
    }
}

async fn stream_once<M: CompletionModel + 'static>(
    agent: &rig::agent::Agent<M>,
    input: &str,
    history: &[Message],
    tx: &AgentEventSender,
    cancel_rx: &mut watch::Receiver<bool>,
    max_multi_turn: u32,
) -> Result<Vec<Message>, TurnFailure> {
    let mut section = AgentSection::Answer;
    let mut partial = PartialTurn::default();
    let mut final_history: Option<Vec<Message>> = None;
    let stream_request = agent
        .stream_prompt(input)
        .history(history.to_vec())
        .max_turns(max_multi_turn as usize);
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
                let _ = tx.send(AgentEvent::ToolResult {
                    text,
                    internal_call_id,
                });
            }
            Some(Ok(MultiTurnStreamItem::FinalResponse(final_response))) => {
                if let Some(updated) = final_response.messages() {
                    final_history = Some(updated.to_vec());
                }
            }
            Some(Ok(_)) => {}
            Some(Err(err)) => {
                let saw_tool_call = partial.saw_tool_call;
                let history = match canonical_history(&err) {
                    Some(h) => h,
                    None => partial.finish(input, history),
                };
                return Err(TurnFailure {
                    history,
                    saw_tool_call,
                    source: AgentError::Stream { source: err },
                });
            }
            None => break,
        }
    }
    Ok(final_history.unwrap_or_else(|| partial.finish(input, history)))
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
        tx: AgentEventSender,
        cancel_rx: watch::Receiver<bool>,
    ) -> Result<Vec<Message>, TurnFailure> {
        (self.chat)(
            input.to_string(),
            history,
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
            $variant:ident : $mod:ident,
            client = $client:ty,
            model  = $model:ty,
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
                            let inner = Arc::new(
                                client.agent(model)
                                    .preamble(preamble)
                                    .tools(tools)
                                    .build(),
                            );
                            return Ok(DynamicAgent {
                                chat: Box::new(
                                    move |input, history: Arc<[Message]>, tx, cancel_rx, max_multi_turn| {
                                        let agent = Arc::clone(&inner);
                                        Box::pin(async move {
                                            crate::agent::stream_chat(
                                                &agent,
                                                &input,
                                                &history,
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
    DeepSeek   : deepseek,
        client = rig::providers::deepseek::Client,
        model  = rig::providers::deepseek::CompletionModel,
        env    = "DEEPSEEK_API_KEY",
        prefixes = ["deepseek-"],
    OpenAI     : openai,
        client = rig::providers::openai::CompletionsClient,
        model  = rig::providers::openai::completion::CompletionModel,
        env    = "OPENAI_API_KEY",
        prefixes = ["gpt-", "o1", "o3", "o4"],
    Anthropic  : anthropic,
        client = rig::providers::anthropic::Client,
        model  = rig::providers::anthropic::completion::CompletionModel,
        env    = "ANTHROPIC_API_KEY",
        prefixes = ["claude-"],
    Gemini     : gemini,
        client = rig::providers::gemini::Client,
        model  = rig::providers::gemini::completion::CompletionModel,
        env    = "GEMINI_API_KEY",
        prefixes = ["gemini-"],
    Xai        : xai,
        client = rig::providers::xai::Client,
        model  = rig::providers::xai::completion::CompletionModel,
        env    = "XAI_API_KEY",
        prefixes = ["grok-"],
    Mistral    : mistral,
        client = rig::providers::mistral::Client,
        model  = rig::providers::mistral::completion::CompletionModel,
        env    = "MISTRAL_API_KEY",
        prefixes = ["mistral-", "ministral-", "codestral-", "pixtral-"],
    Cohere     : cohere,
        client = rig::providers::cohere::Client,
        model  = rig::providers::cohere::completion::CompletionModel,
        env    = "COHERE_API_KEY",
        prefixes = ["command-"],
    Perplexity : perplexity,
        client = rig::providers::perplexity::Client,
        model  = rig::providers::perplexity::CompletionModel,
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
        assert_eq!(retry_decision(&err, true, 1), RetryDecision::Abort);
    }

    #[test]
    fn retry_allowed_before_any_tool_call() {
        let err = stream_err(http::StatusCode::SERVICE_UNAVAILABLE);
        assert!(matches!(
            retry_decision(&err, false, 1),
            RetryDecision::Retry { .. }
        ));
    }

    #[test]
    fn retry_aborts_when_attempts_exhausted() {
        let err = stream_err(http::StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            retry_decision(&err, false, MAX_ATTEMPTS),
            RetryDecision::Abort
        );
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
}
