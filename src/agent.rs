use crate::error::{ErrorKind, TogiError};
use futures::StreamExt;
use rig::agent::MultiTurnStreamItem;
use rig::client::{CompletionClient, ProviderClient};
use rig::completion::CompletionModel;
use rig::completion::message::ToolResultContent;
use rig::message::Message;
use rig::streaming::{StreamedAssistantContent, StreamedUserContent, StreamingChat};
use rig::tool::ToolDyn;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::watch;

type DynError = Box<dyn std::error::Error + Send + Sync + 'static>;

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("无法识别模型 \"{model}\" 对应的提供商。\n支持的提供商：{supported}")]
    UnknownProvider { model: String, supported: String },

    #[error("未设置 {env} 环境变量：{source}")]
    MissingApiKey {
        env: &'static str,
        #[source]
        source: DynError,
    },

    #[error("无法初始化 {provider} 提供商：{source}")]
    ProviderInit {
        provider: &'static str,
        #[source]
        source: DynError,
    },

    #[error("模型流式响应失败：{source}")]
    Stream {
        #[source]
        source: DynError,
    },
}

impl TogiError for AgentError {
    fn code(&self) -> &'static str {
        match self {
            Self::UnknownProvider { .. } => "agent.unknown_provider",
            Self::MissingApiKey { .. } => "agent.missing_api_key",
            Self::ProviderInit { .. } => "agent.provider_init",
            Self::Stream { .. } => "agent.stream",
        }
    }

    fn kind(&self) -> ErrorKind {
        match self {
            Self::UnknownProvider { .. } => ErrorKind::InvalidArgument,
            Self::MissingApiKey { .. } => ErrorKind::MissingRuntimeInjection,
            Self::ProviderInit { .. } | Self::Stream { .. } => ErrorKind::External,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentSection {
    Reasoning,
    Answer,
}

#[derive(Debug, Clone)]
pub enum AgentEvent {
    Section(AgentSection),
    Text(String),
    ToolCall {
        name: String,
        arguments: serde_json::Value,
    },
    ToolResult(String),
    Notice(String),
}

type AgentEventSender = tokio::sync::mpsc::UnboundedSender<AgentEvent>;
type ChatFuture = Pin<Box<dyn Future<Output = Result<Vec<Message>, AgentError>> + Send>>;
type ChatFn = dyn Fn(String, Vec<Message>, AgentEventSender, watch::Receiver<bool>, u32) -> ChatFuture
    + Send
    + Sync;

fn cancel_return(tx: &AgentEventSender, history: &[Message]) -> Result<Vec<Message>, AgentError> {
    let _ = tx.send(AgentEvent::Notice("Esc 已中断当前回答。".to_string()));
    Ok(history.to_vec())
}

fn ensure_section(current: &mut AgentSection, target: AgentSection, tx: &AgentEventSender) {
    if *current != target {
        *current = target;
        let _ = tx.send(AgentEvent::Section(target));
    }
}
pub async fn stream_chat<M: CompletionModel + 'static>(
    agent: &rig::agent::Agent<M>,
    input: &str,
    history: &[Message],
    tx: AgentEventSender,
    mut cancel_rx: watch::Receiver<bool>,
    max_multi_turn: u32,
) -> Result<Vec<Message>, AgentError> {
    let mut section = AgentSection::Answer;
    let mut final_history: Option<Vec<Message>> = None;
    let stream_request = agent
        .stream_chat(input, history.to_vec())
        .multi_turn(max_multi_turn as usize);
    let mut stream = tokio::select! {
        biased;
        _ = cancel_rx.changed() => return cancel_return(&tx, history),
        stream = stream_request => stream,
    };
    loop {
        let item = tokio::select! {
            biased;
            _ = cancel_rx.changed() => return cancel_return(&tx, history),
            item = stream.next() => item,
        };
        match item {
            Some(Ok(MultiTurnStreamItem::StreamAssistantItem(content))) => match content {
                StreamedAssistantContent::Reasoning(reasoning) => {
                    ensure_section(&mut section, AgentSection::Reasoning, &tx);
                    let _ = tx.send(AgentEvent::Text(reasoning.display_text().to_string()));
                }
                StreamedAssistantContent::ReasoningDelta { reasoning, .. } => {
                    ensure_section(&mut section, AgentSection::Reasoning, &tx);
                    let _ = tx.send(AgentEvent::Text(reasoning));
                }
                StreamedAssistantContent::Text(text) => {
                    ensure_section(&mut section, AgentSection::Answer, &tx);
                    let _ = tx.send(AgentEvent::Text(text.text));
                }
                StreamedAssistantContent::ToolCall { tool_call, .. } => {
                    let _ = tx.send(AgentEvent::ToolCall {
                        name: tool_call.function.name,
                        arguments: tool_call.function.arguments,
                    });
                }
                _ => {}
            },
            Some(Ok(MultiTurnStreamItem::StreamUserItem(StreamedUserContent::ToolResult {
                tool_result,
                ..
            }))) => {
                let text: String = tool_result
                    .content
                    .iter()
                    .filter_map(|c| match c {
                        ToolResultContent::Text(t) => Some(t.text.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let _ = tx.send(AgentEvent::ToolResult(text));
            }
            Some(Ok(MultiTurnStreamItem::FinalResponse(final_response))) => {
                if let Some(updated) = final_response.history() {
                    final_history = Some(updated.to_vec());
                }
            }
            Some(Ok(_)) => {}
            Some(Err(err)) => {
                return Err(AgentError::Stream {
                    source: Box::new(err),
                });
            }
            None => break,
        }
    }
    Ok(final_history.unwrap_or_else(|| history.to_vec()))
}

pub struct DynamicAgent {
    chat: Box<ChatFn>,
    max_multi_turn: u32,
}

impl DynamicAgent {
    pub async fn stream_chat(
        &self,
        input: &str,
        history: Vec<Message>,
        tx: AgentEventSender,
        cancel_rx: watch::Receiver<bool>,
    ) -> Result<Vec<Message>, AgentError> {
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
            parts.join("，")
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
                                    source: Box::new(source),
                                })?
                            } else {
                                <$client>::from_env().map_err(|source| AgentError::MissingApiKey {
                                    env: $env,
                                    source: Box::new(source),
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
                                    move |input, history, tx, cancel_rx, max_multi_turn| {
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
        let result = DynamicAgent::build("unknown-model", "", vec![], None, 3);
        assert!(result.is_err());
        let err_msg = format!("{}", result.err().unwrap());
        assert!(
            err_msg.contains("无法识别模型"),
            "unexpected error: {err_msg}"
        );
    }
}
