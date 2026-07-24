//! 子代理工具（路线图 §1.1）：子代理就是一个工具。
//!
//! `agent` 工具内部构建受限的 [`DynamicAgent`]、复用 `stream_chat` 运行时，
//! 以独立历史完成任务，只把最终结论作为工具结果返回父代理。
//! 子代理定义为 `.togi/agents/<name>.md`（TOML frontmatter 声明模型与工具
//! 白名单，正文作为 preamble）；未指定 profile 时给默认只读工具集
//! （`read` + `shell`，不含 `agent` 自身 → 天然防递归）。

mod profile;

pub use profile::Profile;
pub use profile::load_profiles;

use crate::agent::{AgentEvent, AgentEventSender, DynamicAgent};
use crate::context::{ContextCheckpoint, ContextInput};
use crate::pipeline::confirm::{ApprovalPolicy, confirm};
use crate::pipeline::inject::{CWD_PARAM, inject};
use crate::pipeline::paginate::paginate;
use crate::shared::constants;
use crate::shared::error::TogiError as _;
use crate::tools::ToolRegistry;
use crate::tools::modify::Modify;
use crate::tools::read::Read;
use crate::tools::shell::Shell;
use itertools::Itertools;
use rig::message::{AssistantContent, Message};
use rig::tool::{Tool, ToolCallExtensions, ToolFailure};
use schemars::JsonSchema;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;

/// 子代理最大深度（主代理深度为 0）：允许 主 → 子(1) → 孙(2)，再深拒绝。
const MAX_SUBAGENT_DEPTH: u32 = 2;

/// 未指定 profile 时的默认工具白名单——不含 `agent` 自身，天然防递归。
const DEFAULT_TOOLS: [&str; 2] = ["read", "shell"];

/// 无 profile 时子代理的默认 preamble。
const DEFAULT_PREAMBLE: &str = "你是被父代理委派的子代理：独立完成给定任务，\
最后只返回结论，不要过程描述。工作目录已注入，相对路径直接可用；\
工具输出按行分页，需要更多内容时用 offset / limit 翻页。";

#[derive(Debug, thiserror::Error)]
pub enum AgentToolError {
    #[error(
        "sub-agent depth limit reached (max {max}); do the remaining work directly instead of delegating further"
    )]
    MaxDepth { max: u32 },
    #[error(
        "the agent tool must run inside a togi agent loop (runtime extensions missing); \
         this is a wiring bug, not a task error"
    )]
    MissingRuntime,
    #[error("unknown sub-agent profile `{name}`. Available profiles: {available}")]
    UnknownProfile { name: String, available: String },
    #[error("profile `{profile}` whitelists unknown tool `{tool}`. Available tools: {available}")]
    UnknownTool {
        profile: String,
        tool: String,
        available: String,
    },
    #[error("failed to initialize sub-agent: {0}")]
    Build(String),
    #[error("sub-agent timed out after {secs}s")]
    Timeout { secs: u64 },
    #[error("sub-agent failed: {0}")]
    Run(String),
    #[error("sub-agent finished without a text conclusion")]
    Empty,
}

#[derive(Deserialize, JsonSchema)]
pub struct AgentArgs {
    /// Complete, self-contained task description. The sub-agent cannot see
    /// this conversation, so include all necessary context (paths, goals,
    /// constraints).
    task: String,
    /// Profile name from `.togi/agents/<name>.md`. Omit for a general-purpose
    /// sub-agent with read/shell tools.
    #[serde(default)]
    profile: Option<String>,
}

/// 子代理工具。每次调用构建一个受限子代理并跑完整个任务。
pub struct AgentTool {
    profiles: Arc<HashMap<String, Profile>>,
    cwd: PathBuf,
    default_model: String,
    api_key: Option<String>,
    max_multi_turn: u32,
    depth: u32,
    approval_policy: ApprovalPolicy,
}

impl AgentTool {
    /// 构建主代理持有的 agent 工具（深度 0）：加载 `cwd/.togi/agents/*.md`。
    pub fn new(
        cwd: &Path,
        default_model: String,
        api_key: Option<String>,
        max_multi_turn: u32,
    ) -> Self {
        Self {
            profiles: Arc::new(load_profiles(&cwd.join(".togi").join("agents"))),
            cwd: cwd.to_path_buf(),
            default_model,
            api_key,
            max_multi_turn,
            depth: 0,
            approval_policy: ApprovalPolicy::default(),
        }
    }

    pub(crate) fn with_approval_policy(mut self, approval_policy: ApprovalPolicy) -> Self {
        self.approval_policy = approval_policy;
        self
    }

    /// 派生下一级子代理的工具（深度 +1，多轮上限减半）。
    fn spawn_child(&self) -> Self {
        Self {
            profiles: Arc::clone(&self.profiles),
            cwd: self.cwd.clone(),
            default_model: self.default_model.clone(),
            api_key: self.api_key.clone(),
            max_multi_turn: (self.max_multi_turn / 2).max(1),
            depth: self.depth + 1,
            approval_policy: self.approval_policy.clone(),
        }
    }

    /// 按 profile 白名单（或默认名单）构建子代理工具集，
    /// 走与主代理相同的 inject(cwd) / paginate / confirm 包装。
    fn build_child_tools(
        &self,
        profile: Option<(&str, &Profile)>,
    ) -> Result<Vec<Box<dyn rig::tool::ToolDyn>>, AgentToolError> {
        let names: Vec<&str> = match profile {
            Some((_, p)) if !p.tools.is_empty() => p.tools.iter().map(String::as_str).collect(),
            _ => DEFAULT_TOOLS.to_vec(),
        };
        let mut tools: Vec<Box<dyn rig::tool::ToolDyn>> = Vec::with_capacity(names.len());
        for name in names.into_iter().unique() {
            tools.push(match name {
                Read::NAME => Box::new(Read),
                Shell::NAME => Box::new(Shell),
                Modify::NAME => Box::new(Modify),
                Self::NAME => Box::new(self.spawn_child()),
                other => {
                    return Err(AgentToolError::UnknownTool {
                        profile: profile.map(|(n, _)| n.to_string()).unwrap_or_default(),
                        tool: other.to_string(),
                        available: [Read::NAME, Shell::NAME, Modify::NAME, Self::NAME].join(", "),
                    });
                }
            });
        }
        let mut registry = ToolRegistry::new();
        registry.register::<Read>();
        registry.register::<Shell>();
        registry.register::<Modify>();
        registry.register::<AgentTool>();
        let tools = inject(
            serde_json::Map::from_iter([(CWD_PARAM.into(), self.cwd.display().to_string().into())]),
            tools,
        );
        let tools = paginate(constants::DEFAULT_PAGE_LINES, tools);
        Ok(confirm(tools, registry, self.approval_policy.clone()))
    }

    async fn run(
        &self,
        args: AgentArgs,
        extensions: &ToolCallExtensions,
    ) -> Result<String, AgentToolError> {
        if self.depth >= MAX_SUBAGENT_DEPTH {
            return Err(AgentToolError::MaxDepth {
                max: MAX_SUBAGENT_DEPTH,
            });
        }
        // 运行时注入：事件通道与取消信号由 stream_chat 经 tool_extensions 传入。
        let parent_tx = extensions
            .get::<AgentEventSender>()
            .cloned()
            .ok_or(AgentToolError::MissingRuntime)?;
        let cancel_rx = extensions
            .get::<watch::Receiver<bool>>()
            .cloned()
            .ok_or(AgentToolError::MissingRuntime)?;
        let profile = match args.profile.as_deref() {
            Some(name) => Some((
                name,
                self.profiles
                    .get(name)
                    .ok_or_else(|| AgentToolError::UnknownProfile {
                        name: name.to_string(),
                        available: self.available_profiles(),
                    })?,
            )),
            None => None,
        };
        let tools = self.build_child_tools(profile)?;
        let model = profile
            .and_then(|(_, p)| p.model.as_deref())
            .unwrap_or(&self.default_model);
        let preamble = profile
            .map(|(_, p)| p.preamble.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(DEFAULT_PREAMBLE);
        let agent = DynamicAgent::build(
            model,
            preamble,
            tools,
            self.api_key.as_deref(),
            // 单次子代理多轮上限减半（spawn_child 逐级复合）。
            (self.max_multi_turn / 2).max(1),
        )
        .map_err(|err| AgentToolError::Build(err.user_message()))?;

        // 事件转发：子代理的工具活动戳记展示深度后转给 UI；
        // 流式文本/分区事件丢弃——最终结论会作为本工具结果完整展示，
        // 混入父回答的 markdown 流会造成错乱。
        let (child_tx, mut child_rx) = tokio::sync::mpsc::unbounded_channel();
        let depth = self.depth + 1;
        let drain = tokio::spawn(async move {
            while let Some(event) = child_rx.recv().await {
                let stamped = match event {
                    AgentEvent::Text(_) | AgentEvent::Section(_) => continue,
                    event => AgentEvent::Child {
                        depth,
                        event: Box::new(event),
                    },
                };
                if parent_tx.send(stamped).is_err() {
                    break;
                }
            }
        });

        let run = agent.stream_chat(
            &args.task,
            Arc::from(Vec::new()),
            ContextInput {
                // 子代理历史短（任务级），不启用滚动压缩。
                policy: None,
                checkpoint: ContextCheckpoint::default(),
            },
            child_tx,
            cancel_rx,
        );
        // 超时兜底：小于父流等待工具结果的停滞宽限（TOOL_RESULT_GRACE），
        // 保证子代理超时先触发、错误作为工具结果回传，而非父流被判停滞。
        // ponytail: 固定复用 MAX_TIMEOUT_SECS；真需要更长时做成 profile 字段。
        let outcome =
            tokio::time::timeout(Duration::from_secs(constants::MAX_TIMEOUT_SECS), run).await;
        // run 结束（含超时 drop）后 child_tx 随之释放，drain 排空剩余事件后退出。
        let _ = drain.await;
        let outcome = outcome
            .map_err(|_| AgentToolError::Timeout {
                secs: constants::MAX_TIMEOUT_SECS,
            })?
            .map_err(|failure| AgentToolError::Run(failure.source.user_message()))?;
        conclusion(&outcome.history).ok_or(AgentToolError::Empty)
    }

    fn available_profiles(&self) -> String {
        if self.profiles.is_empty() {
            return "(none — add .togi/agents/<name>.md)".to_string();
        }
        self.profiles.keys().sorted().join(", ")
    }
}

/// 从子代理最终历史中提取结论：最近一条含文本的 assistant 消息。
fn conclusion(history: &[Message]) -> Option<String> {
    history.iter().rev().find_map(|msg| {
        let Message::Assistant { content, .. } = msg else {
            return None;
        };
        let text = content
            .iter()
            .filter_map(|c| match c {
                AssistantContent::Text(t) => Some(t.text.as_str()),
                _ => None,
            })
            .join("\n");
        (!text.trim().is_empty()).then_some(text)
    })
}

impl crate::tools::ClassifyEffect for AgentTool {
    fn name() -> &'static str {
        Self::NAME
    }

    fn classify(_args: &serde_json::Value) -> crate::tools::ToolEffect {
        // 委派本身不产生副作用；子代理的变更工具会各自请求确认。
        crate::tools::ToolEffect::ReadOnlyVerbose
    }
}

impl Tool for AgentTool {
    const NAME: &'static str = "agent";
    type Error = AgentToolError;
    type Args = AgentArgs;
    type Output = String;

    fn description(&self) -> String {
        let mut d = "Delegate a self-contained subtask to a sub-agent. It runs with its own \
             conversation and a restricted tool set, and cannot see this conversation — \
             put everything it needs into `task`. Only its final conclusion is returned \
             as the tool result. Use it to isolate or parallelize well-scoped work \
             (e.g. analyze a directory, review a file)."
            .to_string();
        if !self.profiles.is_empty() {
            d += " Available `profile` values:";
            for (name, p) in self.profiles.iter().sorted_by(|a, b| a.0.cmp(b.0)) {
                let desc = if p.description.is_empty() {
                    "(no description)"
                } else {
                    &p.description
                };
                d += &format!(" {name} — {desc};");
            }
        }
        d
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(AgentArgs)).unwrap()
    }

    async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
        Err(AgentToolError::MissingRuntime)
    }

    async fn call_with_extensions(
        &self,
        args: Self::Args,
        extensions: &ToolCallExtensions,
    ) -> Result<Self::Output, Self::Error> {
        self.run(args, extensions).await
    }

    fn classify_error(&self, error: &Self::Error) -> ToolFailure {
        match error {
            AgentToolError::UnknownProfile { .. } | AgentToolError::UnknownTool { .. } => {
                ToolFailure::invalid_args(error.to_string())
            }
            AgentToolError::Timeout { .. } => ToolFailure::timeout(error.to_string()),
            _ => ToolFailure::other(error.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(depth: u32, profiles: HashMap<String, Profile>) -> AgentTool {
        AgentTool {
            profiles: Arc::new(profiles),
            cwd: PathBuf::from("/tmp"),
            default_model: "deepseek-chat".into(),
            api_key: None,
            max_multi_turn: 10,
            depth,
            approval_policy: ApprovalPolicy::default(),
        }
    }

    fn profile(tools: &[&str]) -> Profile {
        Profile {
            description: "d".into(),
            model: None,
            tools: tools.iter().map(|s| s.to_string()).collect(),
            preamble: "p".into(),
        }
    }

    /// 返回扩展与 cancel sender——sender 必须活到调用结束：watch sender
    /// 全部 drop 后 `changed()` 立即就绪，子代理会被瞬时取消。
    fn extensions() -> (ToolCallExtensions, watch::Sender<bool>) {
        let mut ext = ToolCallExtensions::new();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        ext.insert(tx as AgentEventSender);
        let (cancel_tx, _cancel_rx) = watch::channel(false);
        ext.insert(cancel_tx.subscribe());
        (ext, cancel_tx)
    }

    fn tool_names(tools: &[Box<dyn rig::tool::ToolDyn>]) -> Vec<String> {
        tools.iter().map(|t| t.name()).sorted().collect()
    }

    #[test]
    fn default_child_tools_are_read_and_shell() {
        let names = tool_names(&tool(0, HashMap::new()).build_child_tools(None).unwrap());
        assert_eq!(names, vec!["read", "shell"]);
    }

    #[test]
    fn profile_whitelist_is_respected_and_deduplicated() {
        let t = tool(0, HashMap::new());
        let p = profile(&["shell", "shell", "modify"]);
        let names = tool_names(&t.build_child_tools(Some(("x", &p))).unwrap());
        assert_eq!(names, vec!["modify", "shell"]);
    }

    #[tokio::test]
    async fn child_mutating_tool_waits_for_parent_approval() {
        let t = tool(0, HashMap::new());
        let p = profile(&["modify"]);
        let tools = t.build_child_tools(Some(("writer", &p))).unwrap();
        let modify = tools.iter().find(|tool| tool.name() == "modify").unwrap();
        let mut extensions = ToolCallExtensions::new();
        let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
        extensions.insert(event_tx as AgentEventSender);
        let (cancel_tx, _) = watch::channel(false);
        extensions.insert(cancel_tx.subscribe());
        let mut call = Box::pin(modify.call_structured(
            serde_json::json!({ "path": "blocked.txt", "content": "x" }).to_string(),
            &extensions,
        ));

        let response = tokio::select! {
            event = event_rx.recv() => match event {
                Some(AgentEvent::ApprovalRequest { response, .. }) => response,
                other => panic!("expected approval request, got {other:?}"),
            },
            result = &mut call => panic!("modify completed before approval: {result:?}"),
        };
        response.send(crate::tools::ApprovalDecision::Deny).unwrap();
        assert!(call.await.outcome().is_denied());
    }

    #[test]
    fn unknown_tool_in_whitelist_is_rejected() {
        let t = tool(0, HashMap::new());
        let p = profile(&["read", "nope"]);
        let err = t
            .build_child_tools(Some(("reader", &p)))
            .map(drop)
            .unwrap_err();
        assert!(matches!(err, AgentToolError::UnknownTool { .. }));
        assert!(err.to_string().contains("nope"));
        assert!(err.to_string().contains("reader"));
    }

    #[test]
    fn nested_agent_tool_gets_next_depth_and_halved_turns() {
        let t = tool(1, HashMap::new());
        let p = profile(&["agent", "read"]);
        let tools = t.build_child_tools(Some(("x", &p))).unwrap();
        let nested = tools.iter().find(|tool| tool.name() == "agent").unwrap();
        let _ = nested; // 深度/轮次无法经 ToolDyn 观察，直接验证 spawn_child。
        let child = t.spawn_child();
        assert_eq!(child.depth, 2);
        assert_eq!(child.max_multi_turn, 5);
    }

    #[tokio::test]
    async fn depth_limit_rejects_before_any_work() {
        let t = tool(MAX_SUBAGENT_DEPTH, HashMap::new());
        let (ext, _cancel_tx) = extensions();
        let err = t
            .run(
                AgentArgs {
                    task: "x".into(),
                    profile: None,
                },
                &ext,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, AgentToolError::MaxDepth { .. }));
    }

    #[tokio::test]
    async fn missing_runtime_extensions_is_an_error() {
        let t = tool(0, HashMap::new());
        let err = t
            .run(
                AgentArgs {
                    task: "x".into(),
                    profile: None,
                },
                &ToolCallExtensions::new(),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, AgentToolError::MissingRuntime));
    }

    #[tokio::test]
    async fn unknown_profile_lists_available() {
        let mut profiles = HashMap::new();
        profiles.insert("reader".to_string(), profile(&[]));
        let t = tool(0, profiles);
        let (ext, _cancel_tx) = extensions();
        let err = t
            .run(
                AgentArgs {
                    task: "x".into(),
                    profile: Some("nope".into()),
                },
                &ext,
            )
            .await
            .unwrap_err();
        let AgentToolError::UnknownProfile { available, .. } = err else {
            panic!("expected UnknownProfile, got {err}");
        };
        assert_eq!(available, "reader");
    }

    #[test]
    fn conclusion_takes_last_assistant_text() {
        let history = vec![
            Message::user("q"),
            Message::assistant("中间结论"),
            Message::user("tool result"),
            Message::assistant("最终结论"),
        ];
        assert_eq!(conclusion(&history).as_deref(), Some("最终结论"));
        assert_eq!(conclusion(&[]), None);
        assert_eq!(conclusion(&[Message::user("q")]), None);
    }

    #[test]
    fn description_lists_profiles() {
        let mut profiles = HashMap::new();
        profiles.insert("reader".to_string(), profile(&[]));
        let d = tool(0, profiles).description();
        assert!(d.contains("reader"));
        let d = tool(0, HashMap::new()).description();
        assert!(!d.contains("profile` values"));
    }
}
